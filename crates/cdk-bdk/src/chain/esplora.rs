use std::time::Instant;

use bdk_esplora::esplora_client::{AsyncClient, Builder};
use bdk_esplora::EsploraAsyncExt;
use bdk_wallet::bitcoin::Transaction;
use bdk_wallet::WalletEvent;
use tokio::time::{interval, Duration};
use tokio_util::sync::CancellationToken;

use crate::chain::EsploraConfig;
use crate::error::Error;
use crate::CdkBdk;

pub(crate) async fn sync_esplora(
    cdk_bdk: &CdkBdk,
    config: &EsploraConfig,
    cancel_token: CancellationToken,
) -> Result<(), Error> {
    let url = &config.url;
    let parallel_requests = config.parallel_requests;
    let mut sync_interval = interval(Duration::from_secs(cdk_bdk.sync_interval_secs));
    let mut startup_reconciliation_pending = true;
    let warn_ms = cdk_bdk.sync_config.lock_hold_warn_ms;

    // Persist Esplora client across sync iterations; re-create on error.
    let mut esplora_client: Option<AsyncClient> = None;
    let mut consecutive_failures: u32 = 0;

    tracing::info!(
        url = %url,
        parallel_requests,
        interval_secs = cdk_bdk.sync_interval_secs,
        "Starting Esplora block sync"
    );
    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                tracing::info!("Wallet sync cancelled via cancel token");
                break;
            }
            _ = sync_interval.tick() => {
                let client = match &esplora_client {
                    Some(c) => c.clone(),
                    None => {
                        match Builder::new(url).build_async() {
                            Ok(c) => {
                                esplora_client = Some(c.clone());
                                c
                            }
                            Err(e) => {
                                consecutive_failures =
                                    consecutive_failures.saturating_add(1);
                                let err = Error::Esplora(e.to_string());
                                crate::sync::log_sync_failure(
                                    "Failed to construct Esplora client",
                                    &err,
                                    consecutive_failures,
                                );
                                continue;
                            }
                        }
                    }
                };

                // Phase A (short lock): build the sync request.
                let sync_request = {
                    let w = cdk_bdk.wallet_with_db.lock().await;
                    w.wallet.start_sync_with_revealed_spks()
                };

                // Phase B (no lock): execute the network sync.
                let sync_update = match client.sync(sync_request, parallel_requests).await {
                    Ok(u) => u,
                    Err(e) => {
                        consecutive_failures =
                            consecutive_failures.saturating_add(1);
                        let err = Error::Esplora(e.to_string());
                        crate::sync::log_sync_failure(
                            "Esplora sync failed",
                            &err,
                            consecutive_failures,
                        );
                        // Drop client so the next tick rebuilds it.
                        esplora_client = None;
                        continue;
                    }
                };

                // Phase C (short lock): apply the update and persist.
                let apply_result: Result<Vec<WalletEvent>, Error> = {
                    let apply_start = Instant::now();
                    let mut w = cdk_bdk.wallet_with_db.lock().await;
                    let res = w
                        .wallet
                        .apply_update_events(sync_update)
                        .map_err(|e| Error::Wallet(e.to_string()))
                        .and_then(|events| {
                            w.persist()?;
                            Ok(events)
                        });
                    let elapsed_ms = apply_start.elapsed().as_millis() as u64;
                    if elapsed_ms > warn_ms {
                        tracing::warn!(
                            held_ms = elapsed_ms,
                            warn_ms,
                            "Wallet lock held longer than configured warning threshold during esplora apply"
                        );
                    }
                    res
                };

                let events = match apply_result {
                    Ok(events) => events,
                    Err(e) => {
                        consecutive_failures =
                            consecutive_failures.saturating_add(1);
                        crate::sync::log_sync_failure(
                            "Failed to apply Esplora update",
                            &e,
                            consecutive_failures,
                        );
                        continue;
                    }
                };

                let tip = {
                    let w = cdk_bdk.wallet_with_db.lock().await;
                    w.wallet.latest_checkpoint().block_id()
                };
                tracing::info!(
                    "Esplora synced to block {} at height {}",
                    tip.hash,
                    tip.height
                );

                if consecutive_failures > 0 {
                    tracing::info!(
                        recovered_after = consecutive_failures,
                        "Esplora sync recovered"
                    );
                    consecutive_failures = 0;
                }

                let has_relevant_events = events.iter().any(|e| matches!(
                    e,
                    WalletEvent::TxConfirmed { .. } | WalletEvent::ChainTipChanged { .. }
                ));

                if startup_reconciliation_pending || has_relevant_events {
                    cdk_bdk.run_reconciliation().await;
                    startup_reconciliation_pending = false;
                }
            }
        }
    }
    Ok(())
}

pub(crate) async fn broadcast_esplora(
    config: &EsploraConfig,
    tx: Transaction,
) -> Result<(), Error> {
    let client = Builder::new(&config.url)
        .build_async()
        .map_err(|e| Error::Esplora(e.to_string()))?;

    tracing::info!(
        "Broadcasting transaction: {} via esplora",
        tx.compute_txid()
    );

    client
        .broadcast(&tx)
        .await
        .map_err(|e| Error::Esplora(e.to_string()))?;

    Ok(())
}

pub(crate) async fn fetch_fee_rate_esplora(
    config: &EsploraConfig,
    target_blocks: u16,
) -> Result<f64, Error> {
    let client = Builder::new(&config.url)
        .build_async()
        .map_err(|e| Error::Esplora(e.to_string()))?;

    let estimates = client
        .get_fee_estimates()
        .await
        .map_err(|e| Error::Esplora(e.to_string()))?;

    // Esplora returns a map of target blocks (as u16) to fee rate (sat/vB as f64)
    if let Some(&rate) = estimates.get(&target_blocks) {
        return Ok(rate);
    }

    // Fallback: find the closest available target block estimate that is >= our target
    let mut available_targets: Vec<u16> = estimates.keys().copied().collect();
    available_targets.sort_unstable();

    for &t in &available_targets {
        if t >= target_blocks {
            if let Some(&rate) = estimates.get(&t) {
                return Ok(rate);
            }
        }
    }

    // If nothing >= target, take the largest available
    if let Some(&t) = available_targets.last() {
        if let Some(&rate) = estimates.get(&t) {
            return Ok(rate);
        }
    }

    Err(Error::FeeEstimationUnavailable)
}

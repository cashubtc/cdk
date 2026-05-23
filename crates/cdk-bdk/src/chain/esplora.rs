use std::collections::HashMap;
use std::sync::{LazyLock, Mutex as StdMutex};
use std::time::Instant;

use bdk_esplora::esplora_client::{AsyncClient, Builder};
use bdk_esplora::EsploraAsyncExt;
use bdk_wallet::bitcoin::{OutPoint, Transaction};
use tokio::time::{interval, Duration};
use tokio_util::sync::CancellationToken;

use crate::chain::{BroadcastErrorKind, BroadcastFailure, BroadcastOutcome, EsploraConfig};
use crate::error::Error;
use crate::CdkBdk;

const MIN_ESPLORA_BACKOFF: Duration = Duration::from_secs(5);
const MAX_ESPLORA_BACKOFF: Duration = Duration::from_secs(300);

fn next_esplora_backoff(backoff: &mut Duration) -> Duration {
    let current = *backoff;
    *backoff = (*backoff * 2).min(MAX_ESPLORA_BACKOFF);
    current
}

/// Shared Esplora clients keyed by URL so per-call helpers reuse one
/// connection pool instead of building a fresh client each call.
static SHARED_ESPLORA_CLIENTS: LazyLock<StdMutex<HashMap<String, AsyncClient>>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));

fn shared_esplora_client(url: &str) -> Result<AsyncClient, Error> {
    let mut clients = SHARED_ESPLORA_CLIENTS
        .lock()
        .map_err(|err| Error::Esplora(format!("Esplora client lock poisoned: {err}")))?;
    if let Some(client) = clients.get(url) {
        return Ok(client.clone());
    }
    let client = Builder::new(url)
        .build_async()
        .map_err(|e| Error::Esplora(e.to_string()))?;
    clients.insert(url.to_string(), client.clone());
    Ok(client)
}

pub(crate) async fn sync_esplora(
    cdk_bdk: &CdkBdk,
    config: &EsploraConfig,
    cancel_token: CancellationToken,
) -> Result<(), Error> {
    let url = &config.url;
    let parallel_requests = config.parallel_requests;
    let configured_interval = Duration::from_secs(cdk_bdk.sync_interval_secs);
    let initial_backoff = configured_interval.max(MIN_ESPLORA_BACKOFF);
    let mut sync_interval = interval(configured_interval);
    let warn_ms = cdk_bdk.sync_config.lock_hold_warn_ms;

    // Persist Esplora client across sync iterations; re-create on error.
    let mut esplora_client: Option<AsyncClient> = None;
    let mut consecutive_failures: u32 = 0;
    let mut backoff = initial_backoff;

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
                                let retry_delay = next_esplora_backoff(&mut backoff);
                                tracing::warn!(
                                    retry_delay_secs = retry_delay.as_secs(),
                                    "Backing off Esplora sync retry"
                                );
                                sync_interval.reset_after(retry_delay);
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
                        let retry_delay = next_esplora_backoff(&mut backoff);
                        tracing::warn!(
                            retry_delay_secs = retry_delay.as_secs(),
                            "Backing off Esplora sync retry"
                        );
                        sync_interval.reset_after(retry_delay);
                        continue;
                    }
                };

                // Phase C (short lock): apply the update and persist.
                let apply_result = {
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

                if let Err(e) = apply_result {
                        consecutive_failures =
                            consecutive_failures.saturating_add(1);
                        crate::sync::log_sync_failure(
                            "Failed to apply Esplora update",
                            &e,
                            consecutive_failures,
                        );
                        continue;
                }

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
                backoff = initial_backoff;

                cdk_bdk.run_reconciliation().await;
            }
        }
    }
    Ok(())
}

pub(crate) fn classify_esplora_broadcast_error(message: &str) -> BroadcastErrorKind {
    let lower = message.to_ascii_lowercase();

    if lower.contains("already")
        && (lower.contains("known") || lower.contains("mempool") || lower.contains("chain"))
    {
        return BroadcastErrorKind::Unknown;
    }

    if lower.contains("dust")
        || lower.contains("min relay")
        || lower.contains("minrelay")
        || lower.contains("mandatory-script-verify-flag-failed")
        || lower.contains("non-mandatory-script-verify-flag")
        || lower.contains("bad-txns")
        || lower.contains("nonstandard")
        || lower.contains("non-standard")
        || lower.contains("insufficient fee")
        || lower.contains("fee too low")
        || lower.contains("mempool min fee")
        || lower.contains("missing inputs")
        || lower.contains("txn-mempool-conflict")
        || lower.contains("replacement-adds-unconfirmed")
    {
        return BroadcastErrorKind::Rejected;
    }

    if lower.contains("timeout")
        || lower.contains("timed out")
        || lower.contains("connection")
        || lower.contains("connect")
        || lower.contains("dns")
        || lower.contains("http")
        || lower.contains("status 5")
        || lower.contains("temporarily unavailable")
        || lower.contains("too many requests")
    {
        return BroadcastErrorKind::Transient;
    }

    BroadcastErrorKind::Unknown
}

pub(crate) async fn broadcast_esplora(
    config: &EsploraConfig,
    tx: Transaction,
) -> Result<BroadcastOutcome, BroadcastFailure> {
    let client = shared_esplora_client(&config.url)
        .map_err(|e| BroadcastFailure::new(BroadcastErrorKind::Transient, e.to_string()))?;

    tracing::info!(
        "Broadcasting transaction: {} via esplora",
        tx.compute_txid()
    );

    match client.broadcast(&tx).await {
        Ok(()) => Ok(BroadcastOutcome::Accepted),
        Err(e) => {
            let message = e.to_string();
            let lower = message.to_ascii_lowercase();
            if lower.contains("already")
                && (lower.contains("known") || lower.contains("mempool") || lower.contains("chain"))
            {
                return Ok(BroadcastOutcome::AlreadyKnown);
            }

            Err(BroadcastFailure::new(
                classify_esplora_broadcast_error(&message),
                message,
            ))
        }
    }
}

pub(crate) async fn fetch_fee_rate_esplora(
    config: &EsploraConfig,
    target_blocks: u16,
) -> Result<f64, Error> {
    let client = shared_esplora_client(&config.url)?;

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

pub(crate) async fn any_confirmed_spend_esplora(
    config: &EsploraConfig,
    outpoints: &[OutPoint],
) -> Result<bool, Error> {
    let client = shared_esplora_client(&config.url)?;

    for outpoint in outpoints {
        let Some(status) = client
            .get_output_status(&outpoint.txid, outpoint.vout.into())
            .await
            .map_err(|e| Error::Esplora(e.to_string()))?
        else {
            continue;
        };

        if status.spent && status.status.is_some_and(|tx_status| tx_status.confirmed) {
            return Ok(true);
        }
    }

    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_esplora_broadcast_errors() {
        assert_eq!(
            classify_esplora_broadcast_error("sendrawtransaction RPC error: missing inputs"),
            BroadcastErrorKind::Rejected
        );
        assert_eq!(
            classify_esplora_broadcast_error("connection timeout"),
            BroadcastErrorKind::Transient
        );
        assert_eq!(
            classify_esplora_broadcast_error("unexpected backend response"),
            BroadcastErrorKind::Unknown
        );
    }
}

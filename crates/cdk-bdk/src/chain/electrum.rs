use std::sync::Arc;
use std::time::Instant;

use bdk_electrum::electrum_client::{Client, ConfigBuilder, ElectrumApi};
use bdk_electrum::BdkElectrumClient;
use bdk_wallet::bitcoin::Transaction;
use tokio::time::{interval, Duration};
use tokio_util::sync::CancellationToken;

use crate::chain::{BroadcastErrorKind, BroadcastFailure, BroadcastOutcome, ElectrumConfig};
use crate::error::Error;
use crate::CdkBdk;

const ELECTRUM_TIMEOUT_SECS: u8 = 10;
const MIN_ELECTRUM_BACKOFF: Duration = Duration::from_secs(5);
const MAX_ELECTRUM_BACKOFF: Duration = Duration::from_secs(300);

type ElectrumClient = BdkElectrumClient<Client>;

fn new_electrum_client(url: &str) -> Result<ElectrumClient, bdk_electrum::electrum_client::Error> {
    let client_config = ConfigBuilder::new()
        .timeout(Some(ELECTRUM_TIMEOUT_SECS))
        .build();
    let client = Client::from_config(url, client_config)?;
    Ok(BdkElectrumClient::new(client))
}

fn next_electrum_backoff(backoff: &mut Duration) -> Duration {
    let current = *backoff;
    *backoff = (*backoff * 2).min(MAX_ELECTRUM_BACKOFF);
    current
}

pub(crate) async fn sync_electrum(
    cdk_bdk: &CdkBdk,
    config: &ElectrumConfig,
    cancel_token: CancellationToken,
) -> Result<(), Error> {
    let configured_interval = Duration::from_secs(cdk_bdk.sync_interval_secs);
    let initial_backoff = configured_interval.max(MIN_ELECTRUM_BACKOFF);
    let mut sync_interval = interval(configured_interval);
    let mut electrum_client: Option<Arc<ElectrumClient>> = None;
    let mut consecutive_failures: u32 = 0;
    let mut backoff = initial_backoff;
    let warn_ms = cdk_bdk.sync_config.lock_hold_warn_ms;

    tracing::info!(
        url = %config.url,
        batch_size = config.batch_size,
        interval_secs = cdk_bdk.sync_interval_secs,
        "Starting Electrum block sync"
    );

    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                tracing::info!("Wallet sync cancelled via cancel token");
                break;
            }
            _ = sync_interval.tick() => {
                let client = match &electrum_client {
                    Some(client) => Arc::clone(client),
                    None => {
                        let url = config.url.clone();
                        match tokio::task::spawn_blocking(move || new_electrum_client(&url)).await {
                        Ok(Ok(client)) => {
                            let client = Arc::new(client);
                            electrum_client = Some(Arc::clone(&client));
                            client
                        }
                        Ok(Err(error)) => {
                            consecutive_failures = consecutive_failures.saturating_add(1);
                            let error = Error::Electrum(error.to_string());
                            crate::sync::log_sync_failure(
                                "Failed to construct Electrum client",
                                &error,
                                consecutive_failures,
                            );
                            let retry_delay = next_electrum_backoff(&mut backoff);
                            tracing::warn!(
                                retry_delay_secs = retry_delay.as_secs(),
                                "Backing off Electrum sync retry"
                            );
                            sync_interval.reset_after(retry_delay);
                            continue;
                        }
                        Err(error) => {
                            consecutive_failures = consecutive_failures.saturating_add(1);
                            let error = Error::Electrum(format!(
                                "Electrum client task failed: {error}"
                            ));
                            crate::sync::log_sync_failure(
                                "Electrum client task failed",
                                &error,
                                consecutive_failures,
                            );
                            let retry_delay = next_electrum_backoff(&mut backoff);
                            sync_interval.reset_after(retry_delay);
                            continue;
                        }
                    }
                    }
                };

                let sync_request = {
                    let wallet = cdk_bdk.wallet_with_db.lock().await;
                    client.populate_tx_cache(
                        wallet.wallet.tx_graph().full_txs().map(|tx_node| tx_node.tx),
                    );
                    wallet.wallet.start_sync_with_revealed_spks()
                };

                let sync_client = Arc::clone(&client);
                let batch_size = config.batch_size;
                let sync_update = match tokio::task::spawn_blocking(move || {
                    sync_client.sync(sync_request, batch_size, false)
                })
                .await
                {
                    Ok(Ok(update)) => update,
                    Ok(Err(error)) => {
                        consecutive_failures = consecutive_failures.saturating_add(1);
                        let error = Error::Electrum(error.to_string());
                        crate::sync::log_sync_failure(
                            "Electrum sync failed",
                            &error,
                            consecutive_failures,
                        );
                        electrum_client = None;
                        let retry_delay = next_electrum_backoff(&mut backoff);
                        tracing::warn!(
                            retry_delay_secs = retry_delay.as_secs(),
                            "Backing off Electrum sync retry"
                        );
                        sync_interval.reset_after(retry_delay);
                        continue;
                    }
                    Err(error) => {
                        consecutive_failures = consecutive_failures.saturating_add(1);
                        let error = Error::Electrum(format!(
                            "Electrum sync task failed: {error}"
                        ));
                        crate::sync::log_sync_failure(
                            "Electrum sync task failed",
                            &error,
                            consecutive_failures,
                        );
                        electrum_client = None;
                        let retry_delay = next_electrum_backoff(&mut backoff);
                        sync_interval.reset_after(retry_delay);
                        continue;
                    }
                };

                let apply_result = {
                    let apply_start = Instant::now();
                    let mut wallet = cdk_bdk.wallet_with_db.lock().await;
                    let result = wallet
                        .wallet
                        .apply_update_events(sync_update)
                        .map_err(|error| Error::Wallet(error.to_string()))
                        .and_then(|events| {
                            wallet.persist()?;
                            Ok(events)
                        });
                    let elapsed_ms = apply_start.elapsed().as_millis() as u64;
                    if elapsed_ms > warn_ms {
                        tracing::warn!(
                            held_ms = elapsed_ms,
                            warn_ms,
                            "Wallet lock held longer than configured warning threshold during Electrum apply"
                        );
                    }
                    result
                };

                if let Err(error) = apply_result {
                    consecutive_failures = consecutive_failures.saturating_add(1);
                    crate::sync::log_sync_failure(
                        "Failed to apply Electrum update",
                        &error,
                        consecutive_failures,
                    );
                    continue;
                }

                let tip = {
                    let wallet = cdk_bdk.wallet_with_db.lock().await;
                    wallet.wallet.latest_checkpoint().block_id()
                };
                tracing::info!(
                    "Electrum synced to block {} at height {}",
                    tip.hash,
                    tip.height
                );

                if consecutive_failures > 0 {
                    tracing::info!(
                        recovered_after = consecutive_failures,
                        "Electrum sync recovered"
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

pub(crate) fn classify_electrum_broadcast_error(message: &str) -> BroadcastErrorKind {
    let message = message.to_ascii_lowercase();

    if message.contains("dust")
        || message.contains("min relay")
        || message.contains("minrelay")
        || message.contains("mandatory-script-verify-flag-failed")
        || message.contains("non-mandatory-script-verify-flag")
        || message.contains("bad-txns")
        || message.contains("nonstandard")
        || message.contains("non-standard")
        || message.contains("insufficient fee")
        || message.contains("fee too low")
        || message.contains("mempool min fee")
        || message.contains("missing inputs")
        || message.contains("txn-mempool-conflict")
    {
        return BroadcastErrorKind::Rejected;
    }

    if message.contains("timeout")
        || message.contains("timed out")
        || message.contains("connection")
        || message.contains("connect")
        || message.contains("dns")
        || message.contains("broken pipe")
        || message.contains("refused")
        || message.contains("reset")
        || message.contains("temporarily unavailable")
    {
        return BroadcastErrorKind::Transient;
    }

    BroadcastErrorKind::Unknown
}

fn is_electrum_already_known(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("already in block chain")
        || message.contains("already in blockchain")
        || message.contains("already have transaction")
        || message.contains("txn-already-in-mempool")
        || message.contains("transaction already in mempool")
}

pub(crate) async fn broadcast_electrum(
    config: &ElectrumConfig,
    tx: Transaction,
) -> Result<BroadcastOutcome, BroadcastFailure> {
    let url = config.url.clone();

    tokio::task::spawn_blocking(move || {
        let client = new_electrum_client(&url).map_err(|error| {
            BroadcastFailure::new(BroadcastErrorKind::Transient, error.to_string())
        })?;

        tracing::info!(
            "Broadcasting transaction: {} via Electrum",
            tx.compute_txid()
        );

        match client.transaction_broadcast(&tx) {
            Ok(_) => Ok(BroadcastOutcome::Accepted),
            Err(error) => {
                let message = error.to_string();
                if is_electrum_already_known(&message) {
                    return Ok(BroadcastOutcome::AlreadyKnown);
                }

                Err(BroadcastFailure::new(
                    classify_electrum_broadcast_error(&message),
                    message,
                ))
            }
        }
    })
    .await
    .map_err(|error| {
        BroadcastFailure::new(
            BroadcastErrorKind::Transient,
            format!("Electrum broadcast task failed: {error}"),
        )
    })?
}

fn btc_per_kb_to_sat_per_vb(rate: f64) -> Option<f64> {
    if !rate.is_finite() || rate <= 0.0 {
        return None;
    }

    let rate = rate * 100_000.0;
    rate.is_finite().then_some(rate)
}

pub(crate) async fn fetch_fee_rate_electrum(
    config: &ElectrumConfig,
    target_blocks: u16,
) -> Result<f64, Error> {
    let url = config.url.clone();

    tokio::task::spawn_blocking(move || {
        let client =
            new_electrum_client(&url).map_err(|error| Error::Electrum(error.to_string()))?;
        let estimate = client
            .inner
            .estimate_fee(target_blocks as usize)
            .map_err(|error| Error::Electrum(error.to_string()))?;

        btc_per_kb_to_sat_per_vb(estimate).ok_or(Error::FeeEstimationUnavailable)
    })
    .await
    .map_err(|error| Error::FeeEstimationFailed(error.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_electrum_broadcast_errors() {
        assert_eq!(
            classify_electrum_broadcast_error("sendrawtransaction RPC error: missing inputs"),
            BroadcastErrorKind::Rejected
        );
        assert_eq!(
            classify_electrum_broadcast_error("connection timeout"),
            BroadcastErrorKind::Transient
        );
        assert_eq!(
            classify_electrum_broadcast_error("unexpected backend response"),
            BroadcastErrorKind::Unknown
        );
    }

    #[test]
    fn detects_already_known_transactions() {
        assert!(is_electrum_already_known("transaction already in mempool"));
        assert!(!is_electrum_already_known("missing inputs"));
    }

    #[test]
    fn converts_electrum_fee_rates() {
        assert_eq!(btc_per_kb_to_sat_per_vb(0.00001), Some(1.0));
        assert_eq!(btc_per_kb_to_sat_per_vb(-1.0), None);
        assert_eq!(btc_per_kb_to_sat_per_vb(0.0), None);
        assert_eq!(btc_per_kb_to_sat_per_vb(f64::NAN), None);
    }
}

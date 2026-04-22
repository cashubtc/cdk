use std::sync::Arc;
use std::time::Instant;

use bdk_bitcoind_rpc::bitcoincore_rpc::{Auth, Client};
use bdk_bitcoind_rpc::{BlockEvent, Emitter, NO_EXPECTED_MEMPOOL_TXS};
use bdk_esplora::esplora_client::{AsyncClient, Builder};
use bdk_esplora::EsploraAsyncExt;
use bdk_wallet::bitcoin::Block;
use bdk_wallet::WalletEvent;
use tokio::sync::Mutex;
use tokio::time::{interval, Duration};
use tokio_util::sync::CancellationToken;

use crate::error::Error;
use crate::{CdkBdk, ChainSource, WalletWithDb};

/// Threshold at which prolonged consecutive failures are escalated from
/// `warn!` to `error!` so operators notice sustained outages.
const SUSTAINED_FAILURE_THRESHOLD: u32 = 10;

/// Apply a chunk of blocks to the wallet under a single lock acquisition,
/// then persist.
async fn apply_and_persist_chunk(
    wallet: &Arc<Mutex<WalletWithDb>>,
    chunk: &mut Vec<BlockEvent<Block>>,
    warn_ms: u64,
) -> Result<(), Error> {
    if chunk.is_empty() {
        return Ok(());
    }

    let start = Instant::now();
    let chunk_len = chunk.len();

    {
        let mut w = wallet.lock().await;
        for block in chunk.drain(..) {
            w.wallet
                .apply_block_connected_to(&block.block, block.block_height(), block.connected_to())
                .map_err(|e| Error::Wallet(e.to_string()))?;
        }
        w.persist()?;
    }

    let elapsed_ms = start.elapsed().as_millis() as u64;
    if elapsed_ms > warn_ms {
        tracing::warn!(
            held_ms = elapsed_ms,
            warn_ms,
            chunk_size = chunk_len,
            "Wallet lock held longer than configured warning threshold during block apply"
        );
    }

    Ok(())
}

/// Log a per-iteration failure at an appropriate severity based on how
/// many consecutive failures have occurred.
fn log_sync_failure(context: &str, err: &Error, consecutive: u32) {
    if consecutive >= SUSTAINED_FAILURE_THRESHOLD {
        tracing::error!(
            consecutive_failures = consecutive,
            transient = err.is_transient(),
            "{context}: {err}"
        );
    } else {
        tracing::warn!(
            consecutive_failures = consecutive,
            transient = err.is_transient(),
            "{context}: {err}"
        );
    }
}

impl CdkBdk {
    /// Run the reconciliation helpers that inspect the wallet after blocks
    /// have been applied. Each helper's failure is logged but does not
    /// tear down the sync task; a subsequent tick will retry naturally.
    async fn run_reconciliation(&self) {
        if let Err(e) = self.scan_for_new_payments().await {
            tracing::warn!(
                transient = e.is_transient(),
                "scan_for_new_payments failed during reconciliation: {e}"
            );
        }
        if let Err(e) = self.check_receive_saga_confirmations().await {
            tracing::warn!(
                transient = e.is_transient(),
                "check_receive_saga_confirmations failed during reconciliation: {e}"
            );
        }
        if let Err(e) = self.check_send_saga_confirmations().await {
            tracing::warn!(
                transient = e.is_transient(),
                "check_send_saga_confirmations failed during reconciliation: {e}"
            );
        }
        if let Err(e) = self.rebroadcast_stuck_batches().await {
            tracing::warn!(
                transient = e.is_transient(),
                "rebroadcast_stuck_batches failed during reconciliation: {e}"
            );
        }
    }

    pub(crate) async fn sync_wallet(&self, cancel_token: CancellationToken) -> Result<(), Error> {
        match &self.chain_source {
            ChainSource::BitcoinRpc(rpc_config) => {
                let mut sync_interval = interval(Duration::from_secs(self.sync_interval_secs));
                let mut startup_reconciliation_pending = true;
                let apply_chunk_size = self.sync_config.apply_chunk_size.max(1);
                let warn_ms = self.sync_config.lock_hold_warn_ms;

                // Persist RPC client across sync iterations; re-create on error.
                let mut rpc_client: Option<Arc<Client>> = None;
                let mut consecutive_failures: u32 = 0;

                tracing::info!(
                    host = %rpc_config.host,
                    port = rpc_config.port,
                    interval_secs = self.sync_interval_secs,
                    "Starting continuous block monitoring via Bitcoin RPC"
                );
                loop {
                    tokio::select! {
                        _ = cancel_token.cancelled() => {
                            tracing::info!("Wallet sync cancelled via cancel token");
                            break;
                        }
                        _ = sync_interval.tick() => {
                            // Lazily construct the RPC client; rebuild on prior error.
                            let client = match &rpc_client {
                                Some(c) => Arc::clone(c),
                                None => {
                                    match Client::new(
                                        &format!("http://{}:{}", rpc_config.host, rpc_config.port),
                                        Auth::UserPass(
                                            rpc_config.user.clone(),
                                            rpc_config.password.clone(),
                                        ),
                                    ) {
                                        Ok(c) => {
                                            let arc = Arc::new(c);
                                            rpc_client = Some(Arc::clone(&arc));
                                            arc
                                        }
                                        Err(e) => {
                                            consecutive_failures =
                                                consecutive_failures.saturating_add(1);
                                            tracing::warn!(
                                                error = %e,
                                                consecutive_failures,
                                                "Failed to construct Bitcoin RPC client; will retry on next tick"
                                            );
                                            continue;
                                        }
                                    }
                                }
                            };

                            // Snapshot the wallet checkpoint under a brief lock.
                            let checkpoint = {
                                let w = self.wallet_with_db.lock().await;
                                w.wallet.latest_checkpoint()
                            };
                            let start_height = checkpoint.height();

                            let mut emitter = Emitter::new(
                                client.as_ref(),
                                checkpoint,
                                start_height,
                                NO_EXPECTED_MEMPOOL_TXS,
                            );

                            let mut any_applied = false;
                            let mut had_tick_error = false;
                            let mut chunk: Vec<BlockEvent<Block>> = Vec::with_capacity(apply_chunk_size);

                            loop {
                                match emitter.next_block() {
                                    Ok(Some(block)) => {
                                        chunk.push(block);
                                        if chunk.len() >= apply_chunk_size {
                                            if let Err(e) = apply_and_persist_chunk(
                                                &self.wallet_with_db,
                                                &mut chunk,
                                                warn_ms,
                                            )
                                            .await
                                            {
                                                had_tick_error = true;
                                                consecutive_failures =
                                                    consecutive_failures.saturating_add(1);
                                                log_sync_failure(
                                                    "Failed to apply block chunk",
                                                    &e,
                                                    consecutive_failures,
                                                );
                                                // Drop the RPC client so it is rebuilt next tick.
                                                rpc_client = None;
                                                break;
                                            }
                                            any_applied = true;
                                        }
                                    }
                                    Ok(None) => break,
                                    Err(e) => {
                                        had_tick_error = true;
                                        consecutive_failures =
                                            consecutive_failures.saturating_add(1);
                                        if consecutive_failures >= SUSTAINED_FAILURE_THRESHOLD {
                                            tracing::error!(
                                                consecutive_failures,
                                                "Bitcoin RPC error during sync: {e}; will retry next tick"
                                            );
                                        } else {
                                            tracing::warn!(
                                                consecutive_failures,
                                                "Bitcoin RPC error during sync: {e}; will retry next tick"
                                            );
                                        }
                                        rpc_client = None;
                                        break;
                                    }
                                }
                            }

                            if !chunk.is_empty() {
                                if let Err(e) = apply_and_persist_chunk(
                                    &self.wallet_with_db,
                                    &mut chunk,
                                    warn_ms,
                                )
                                .await
                                {
                                    had_tick_error = true;
                                    consecutive_failures =
                                        consecutive_failures.saturating_add(1);
                                    log_sync_failure(
                                        "Failed to apply final block chunk",
                                        &e,
                                        consecutive_failures,
                                    );
                                    rpc_client = None;
                                } else {
                                    any_applied = true;
                                }
                            }

                            if any_applied {
                                let tip = {
                                    let w = self.wallet_with_db.lock().await;
                                    w.wallet.latest_checkpoint().block_id()
                                };
                                tracing::info!(
                                    "Synced to new tip {} at height {}",
                                    tip.hash,
                                    tip.height
                                );
                            }

                            if !had_tick_error {
                                if consecutive_failures > 0 {
                                    tracing::info!(
                                        recovered_after = consecutive_failures,
                                        "Bitcoin RPC sync recovered"
                                    );
                                }
                                consecutive_failures = 0;
                            }

                            if startup_reconciliation_pending || any_applied {
                                self.run_reconciliation().await;
                                startup_reconciliation_pending = false;
                            }
                        }
                    }
                }
            }
            ChainSource::Esplora {
                url,
                parallel_requests,
            } => {
                let mut sync_interval = interval(Duration::from_secs(self.sync_interval_secs));
                let mut startup_reconciliation_pending = true;
                let warn_ms = self.sync_config.lock_hold_warn_ms;

                // Persist Esplora client across sync iterations; re-create on error.
                let mut esplora_client: Option<AsyncClient> = None;
                let mut consecutive_failures: u32 = 0;

                tracing::info!(
                    url = %url,
                    parallel_requests = *parallel_requests,
                    interval_secs = self.sync_interval_secs,
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
                                            log_sync_failure(
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
                                let w = self.wallet_with_db.lock().await;
                                w.wallet.start_sync_with_revealed_spks()
                            };

                            // Phase B (no lock): execute the network sync.
                            let sync_update = match client
                                .sync(sync_request, *parallel_requests)
                                .await
                            {
                                Ok(u) => u,
                                Err(e) => {
                                    consecutive_failures =
                                        consecutive_failures.saturating_add(1);
                                    let err = Error::Esplora(e.to_string());
                                    log_sync_failure(
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
                                let mut w = self.wallet_with_db.lock().await;
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
                                    log_sync_failure(
                                        "Failed to apply Esplora update",
                                        &e,
                                        consecutive_failures,
                                    );
                                    continue;
                                }
                            };

                            let tip = {
                                let w = self.wallet_with_db.lock().await;
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
                                self.run_reconciliation().await;
                                startup_reconciliation_pending = false;
                            }
                        }
                    }
                }
            }
        };

        Ok(())
    }
}

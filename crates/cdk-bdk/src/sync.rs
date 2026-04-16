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

impl CdkBdk {
    pub(crate) async fn sync_wallet(&self, cancel_token: CancellationToken) -> Result<(), Error> {
        match &self.chain_source {
            ChainSource::BitcoinRpc(rpc_config) => {
                let mut sync_interval = interval(Duration::from_secs(self.sync_interval_secs));
                let mut startup_reconciliation_pending = true;
                let apply_chunk_size = self.sync_config.apply_chunk_size.max(1);
                let warn_ms = self.sync_config.lock_hold_warn_ms;

                // Persist RPC client across sync iterations; re-create on error.
                let mut rpc_client: Option<Arc<Client>> = None;

                tracing::info!("Starting continuous block monitoring...");
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
                                            tracing::warn!(
                                                error = %e,
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
                                                tracing::error!(
                                                    "Failed to apply block chunk: {}",
                                                    e
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
                                        tracing::warn!(
                                            "RPC error during sync: {}; will retry next tick",
                                            e
                                        );
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
                                    tracing::error!("Failed to apply final block chunk: {}", e);
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

                            if startup_reconciliation_pending || any_applied {
                                self.scan_for_new_payments().await?;
                                self.check_receive_saga_confirmations().await?;
                                self.check_send_saga_confirmations().await?;
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

                tracing::info!("Starting esplora block sync...");
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
                                            tracing::warn!(
                                                error = %e,
                                                "Failed to construct Esplora client; will retry on next tick"
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
                            let sync_update = match client.sync(sync_request, *parallel_requests).await {
                                Ok(u) => u,
                                Err(e) => {
                                    tracing::warn!(
                                        error = %e,
                                        "Esplora sync failed; will retry on next tick"
                                    );
                                    esplora_client = None;
                                    continue;
                                }
                            };

                            // Phase C (short lock): apply the update and persist.
                            let events = {
                                let apply_start = Instant::now();
                                let mut w = self.wallet_with_db.lock().await;
                                let events = w
                                    .wallet
                                    .apply_update_events(sync_update)
                                    .map_err(|e| Error::Wallet(e.to_string()))?;
                                w.persist()?;
                                let elapsed_ms = apply_start.elapsed().as_millis() as u64;
                                if elapsed_ms > warn_ms {
                                    tracing::warn!(
                                        held_ms = elapsed_ms,
                                        warn_ms,
                                        "Wallet lock held longer than configured warning threshold during esplora apply"
                                    );
                                }
                                events
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

                            let has_relevant_events = events.iter().any(|e| matches!(
                                e,
                                WalletEvent::TxConfirmed { .. } | WalletEvent::ChainTipChanged { .. }
                            ));

                            if startup_reconciliation_pending || has_relevant_events {
                                self.scan_for_new_payments().await?;
                                self.check_receive_saga_confirmations().await?;
                                self.check_send_saga_confirmations().await?;
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

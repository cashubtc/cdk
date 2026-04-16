use std::sync::Arc;
use std::time::Instant;

use bdk_bitcoind_rpc::bitcoincore_rpc::{Auth, Client, RawTx, RpcApi};
use bdk_bitcoind_rpc::{BlockEvent, Emitter, NO_EXPECTED_MEMPOOL_TXS};
use bdk_wallet::bitcoin::{Block, Transaction};
use tokio::sync::Mutex;
use tokio::time::{interval, Duration};
use tokio_util::sync::CancellationToken;

use crate::chain::BitcoinRpcConfig;
use crate::error::Error;
use crate::{CdkBdk, WalletWithDb};

/// Apply a chunk of blocks to the wallet under a single lock acquisition,
/// then persist.
pub(crate) async fn apply_and_persist_chunk(
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

pub(crate) async fn sync_bitcoin_rpc(
    cdk_bdk: &CdkBdk,
    config: &BitcoinRpcConfig,
    cancel_token: CancellationToken,
) -> Result<(), Error> {
    let mut sync_interval = interval(Duration::from_secs(cdk_bdk.sync_interval_secs));
    let mut startup_reconciliation_pending = true;
    let apply_chunk_size = cdk_bdk.sync_config.apply_chunk_size.max(1);
    let warn_ms = cdk_bdk.sync_config.lock_hold_warn_ms;

    // Persist RPC client across sync iterations; re-create on error.
    let mut rpc_client: Option<Arc<Client>> = None;
    let mut consecutive_failures: u32 = 0;

    tracing::info!(
        host = %config.host,
        port = config.port,
        interval_secs = cdk_bdk.sync_interval_secs,
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
                            &format!("http://{}:{}", config.host, config.port),
                            Auth::UserPass(
                                config.user.clone(),
                                config.password.clone(),
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
                    let w = cdk_bdk.wallet_with_db.lock().await;
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
                let mut chunk: Vec<bdk_bitcoind_rpc::BlockEvent<Block>> = Vec::with_capacity(apply_chunk_size);

                loop {
                    match emitter.next_block() {
                        Ok(Some(block)) => {
                            chunk.push(block);
                            if chunk.len() >= apply_chunk_size {
                                if let Err(e) = apply_and_persist_chunk(
                                    &cdk_bdk.wallet_with_db,
                                    &mut chunk,
                                    warn_ms,
                                )
                                .await
                                {
                                    had_tick_error = true;
                                    consecutive_failures =
                                        consecutive_failures.saturating_add(1);
                                    crate::sync::log_sync_failure(
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
                            if consecutive_failures >= crate::sync::SUSTAINED_FAILURE_THRESHOLD {
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
                        &cdk_bdk.wallet_with_db,
                        &mut chunk,
                        warn_ms,
                    )
                    .await
                    {
                        had_tick_error = true;
                        consecutive_failures =
                            consecutive_failures.saturating_add(1);
                        crate::sync::log_sync_failure(
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
                        let w = cdk_bdk.wallet_with_db.lock().await;
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
                    cdk_bdk.run_reconciliation().await;
                    startup_reconciliation_pending = false;
                }
            }
        }
    }
    Ok(())
}

pub(crate) async fn broadcast_bitcoin_rpc(
    config: &BitcoinRpcConfig,
    tx: Transaction,
) -> Result<(), Error> {
    let rpc_client: Client = Client::new(
        &format!("http://{}:{}", config.host, config.port),
        Auth::UserPass(config.user.clone(), config.password.clone()),
    )?;

    tracing::info!(
        "Broadcasting transaction: {} via bitcoin rpc",
        tx.compute_txid()
    );

    rpc_client.send_raw_transaction(tx.raw_hex())?;

    Ok(())
}

pub(crate) async fn fetch_fee_rate_bitcoin_rpc(
    config: &BitcoinRpcConfig,
    target_blocks: u16,
) -> Result<f64, Error> {
    // Use a blocking spawn since Client is synchronous
    let config = config.clone();
    let host = config.host.clone();
    let port = config.port;

    tokio::task::spawn_blocking(move || {
        let rpc_client = Client::new(
            &format!("http://{}:{}", host, port),
            Auth::UserPass(config.user, config.password),
        )?;

        let estimate = rpc_client.estimate_smart_fee(target_blocks, None)?;

        if let Some(fee_rate_btc_per_kvb) = estimate.fee_rate {
            // convert BTC/kvB to sat/vB:
            // 1 BTC = 100,000,000 sat
            // 1 kvB = 1,000 vB
            // sat/vB = (BTC/kvB * 100,000,000) / 1,000 = BTC/kvB * 100_000
            let sat_per_vb = fee_rate_btc_per_kvb.to_btc() * 100_000.0;
            Ok(sat_per_vb)
        } else {
            Err(Error::FeeEstimationUnavailable)
        }
    })
    .await
    .map_err(|e| Error::FeeEstimationFailed(e.to_string()))?
}

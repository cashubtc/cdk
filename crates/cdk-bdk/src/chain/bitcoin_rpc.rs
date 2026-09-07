use std::sync::Arc;
use std::time::Instant;

use bdk_bitcoind_rpc::bitcoincore_rpc::json::GetBlockHeaderResult;
use bdk_bitcoind_rpc::bitcoincore_rpc::{Auth, Client, Error as BitcoinRpcError, RawTx, RpcApi};
use bdk_bitcoind_rpc::BlockEvent;
use bdk_wallet::bitcoin::{Block, Transaction};
use bdk_wallet::chain::{BlockId, CheckPoint};
use bdk_wallet::Update;
use tokio::sync::Mutex;
use tokio::time::{interval, Duration, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

use crate::chain::{BitcoinRpcConfig, BroadcastErrorKind, BroadcastFailure, BroadcastOutcome};
use crate::error::Error;
use crate::{CdkBdk, WalletWithDb};

pub(crate) fn initial_checkpoint(config: &BitcoinRpcConfig) -> Result<BlockId, Error> {
    let client = Client::new(
        &format!("http://{}:{}", config.host, config.port),
        Auth::UserPass(config.user.clone(), config.password.clone()),
    )
    .map_err(|source| Error::ChainTipFetchFailed { source })?;
    let chain_info = client
        .get_blockchain_info()
        .map_err(|source| Error::ChainTipFetchFailed { source })?;
    let tip = u32::try_from(chain_info.blocks).map_err(|_| {
        Error::InvalidConfig(format!(
            "Bitcoin Core tip height {} exceeds the supported u32 range",
            chain_info.blocks
        ))
    })?;
    let height = config.wallet_rescan_from_height.unwrap_or(tip);

    if height > tip {
        return Err(Error::WalletRescanHeightTooHigh {
            requested: height,
            tip,
        });
    }

    let hash = if height == tip {
        chain_info.best_block_hash
    } else {
        client.get_block_hash(u64::from(height))?
    };

    Ok(BlockId { height, hash })
}

/// A replacement block may need additional old-chain checkpoints so BDK can
/// invalidate an orphaned tip even when the new block is below its height.
struct RpcBlock {
    event: BlockEvent<Block>,
    connection: Option<CheckPoint>,
}

/// Fetch blocks using headers to recover ancestors missing from sparse wallet
/// checkpoints. The configured birthday only determines the initial checkpoint.
struct RpcBlockFetcher<C> {
    client: Arc<C>,
    checkpoint: CheckPoint,
    last_header: Option<GetBlockHeaderResult>,
}

impl<C> RpcBlockFetcher<C>
where
    C: RpcApi,
{
    fn next_block(&mut self, cancel: &CancellationToken) -> Result<Option<RpcBlock>, Error> {
        if cancel.is_cancelled() {
            return Ok(None);
        }
        let mut block_id = self.checkpoint.block_id();
        let mut header = match self.last_header.take() {
            Some(header) => header,
            None => self.client.get_block_header_info(&block_id.hash)?,
        };
        let mut orphaned = Vec::new();
        loop {
            if cancel.is_cancelled() {
                return Ok(None);
            }
            if header.hash != block_id.hash || header.height != block_id.height as usize {
                return Err(Error::Wallet(format!(
                    "Bitcoin RPC returned an inconsistent header for {} at height {}",
                    block_id.hash, block_id.height
                )));
            }
            if header.confirmations >= 0 {
                break;
            }
            orphaned.push(block_id);
            block_id = BlockId {
                height: block_id.height.checked_sub(1).ok_or_else(|| {
                    Error::Wallet("Bitcoin RPC reports an orphaned genesis block".to_owned())
                })?,
                hash: header.previous_block_hash.ok_or_else(|| {
                    Error::Wallet("Bitcoin RPC header is missing its parent hash".to_owned())
                })?,
            };
            // Do not jump to the next sparse checkpoint (possibly genesis).
            // Walk the old branch's headers to the actual common ancestor.
            header = self.client.get_block_header_info(&block_id.hash)?;
        }

        let next_hash = match header.next_block_hash {
            Some(hash) => hash,
            None if orphaned.is_empty() => return Ok(None),
            None => {
                // BDK needs a conflicting replacement block to invalidate the
                // old tip. Do not reconcile against it after a pure rollback.
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "Bitcoin RPC reorg has no replacement block yet; retry sync",
                )
                .into());
            }
        };
        let next_header = self.client.get_block_header_info(&next_hash)?;
        if cancel.is_cancelled() {
            return Ok(None);
        }
        let next_height = block_id
            .height
            .checked_add(1)
            .ok_or_else(|| Error::Wallet("Bitcoin RPC block height exceeds u32".to_owned()))?;
        if next_header.hash != next_hash
            || next_header.confirmations < 0
            || next_header.previous_block_hash != Some(block_id.hash)
            || next_header.height != next_height as usize
        {
            // The chain changed between RPC calls. Keep any earlier fetched
            // blocks, then rediscover the fork from the persisted tip next tick.
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "Bitcoin RPC chain changed during block fetch; retry sync",
            )
            .into());
        }
        let block = self.client.get_block(&next_hash)?;
        if block.block_hash() != next_hash || block.header.prev_blockhash != block_id.hash {
            return Err(Error::Wallet(
                "Bitcoin RPC returned an inconsistent block".to_owned(),
            ));
        }

        let (base, connection) = match orphaned.is_empty() {
            true => (self.checkpoint.clone(), None),
            false => {
                let base = self.checkpoint.floor_at(block_id.height).ok_or_else(|| {
                    Error::Wallet("Wallet has no checkpoint below the reorg".to_owned())
                })?;
                if base.height() == block_id.height && base.hash() != block_id.hash {
                    return Err(Error::Wallet(
                        "Bitcoin RPC ancestry conflicts with wallet checkpoint".to_owned(),
                    ));
                }
                let base = base.insert(block_id);
                let connection = base
                    .clone()
                    .extend(orphaned.into_iter().rev())
                    .map_err(|_| {
                        Error::Wallet("Bitcoin RPC ancestors are out of order".to_owned())
                    })?;
                (base, Some(connection))
            }
        };
        self.checkpoint = base
            .push(BlockId {
                height: next_height,
                hash: next_hash,
            })
            .map_err(|_| Error::Wallet("Bitcoin RPC blocks are out of order".to_owned()))?;
        self.last_header = Some(next_header);
        Ok(Some(RpcBlock {
            event: BlockEvent {
                block,
                checkpoint: self.checkpoint.clone(),
            },
            connection,
        }))
    }
}

/// Apply a chunk of blocks to the wallet under a single lock acquisition,
/// then persist.
async fn apply_and_persist_chunk(
    wallet: &Arc<Mutex<WalletWithDb>>,
    chunk: &mut Vec<RpcBlock>,
    warn_ms: u64,
) -> Result<(), Error> {
    let chunk_len = chunk.len();

    let elapsed_ms;
    {
        let mut w = wallet.lock().await;
        let start = Instant::now();
        for block in chunk.drain(..) {
            if let Some(connection) = block.connection {
                w.wallet
                    .apply_update(Update {
                        chain: Some(connection),
                        ..Default::default()
                    })
                    .map_err(|error| Error::Wallet(error.to_string()))?;
            }
            let block = block.event;
            w.wallet
                .apply_block_connected_to(&block.block, block.block_height(), block.connected_to())
                .map_err(|e| Error::Wallet(e.to_string()))?;
        }
        w.persist()?;
        elapsed_ms = start.elapsed().as_millis() as u64;
    }

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

/// Fetch blocks without occupying a runtime worker or holding the wallet lock.
/// Returns whether any blocks were applied during this tick.
async fn sync_rpc_once<C>(
    wallet: &Arc<Mutex<WalletWithDb>>,
    client: Arc<C>,
    chunk_size: usize,
    warn_ms: u64,
    cancel_token: &CancellationToken,
) -> Result<bool, Error>
where
    C: RpcApi + Send + Sync + 'static,
{
    // Also stop fetching if the supervisor drops this future. A running
    // blocking RPC call cannot be aborted, but it never owns the wallet.
    let cancel = cancel_token.child_token();
    let _cancel_on_drop = cancel.clone().drop_guard();

    // A prior apply may have advanced the in-memory checkpoint before a
    // database error. Retry those staged changes even when there are no blocks.
    apply_and_persist_chunk(wallet, &mut Vec::new(), warn_ms).await?;
    let checkpoint = wallet.lock().await.wallet.latest_checkpoint();
    let mut fetcher = RpcBlockFetcher {
        client,
        checkpoint,
        last_header: None,
    };
    let chunk_size = chunk_size.max(1);
    let mut any_applied = false;

    loop {
        if cancel.is_cancelled() {
            return Ok(false);
        }
        let fetch_cancel = cancel.clone();
        let fetch = tokio::task::spawn_blocking(move || {
            let mut chunk = Vec::with_capacity(chunk_size);
            let result = (|| {
                while chunk.len() < chunk_size && !fetch_cancel.is_cancelled() {
                    match fetcher.next_block(&fetch_cancel)? {
                        Some(block) => chunk.push(block),
                        None => return Ok(true),
                    }
                }
                Ok::<_, Error>(false)
            })();
            (fetcher, chunk, result)
        });

        let (next_fetcher, mut chunk, result) = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(false),
            result = fetch => result.map_err(std::io::Error::other)?,
        };
        fetcher = next_fetcher;
        any_applied |= !chunk.is_empty();
        // Preserve successfully fetched blocks even if a later RPC failed.
        apply_and_persist_chunk(wallet, &mut chunk, warn_ms).await?;
        if result? {
            return Ok(any_applied);
        }
    }
}

pub(crate) async fn sync_bitcoin_rpc(
    cdk_bdk: &CdkBdk,
    config: &BitcoinRpcConfig,
    cancel_token: CancellationToken,
) -> Result<(), Error> {
    let mut sync_interval = interval(Duration::from_secs(cdk_bdk.sync_interval_secs));
    // Skip the backlog after a slow sync to avoid catch-up bursts.
    sync_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
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

                let result = sync_rpc_once(
                    &cdk_bdk.wallet_with_db,
                    client,
                    apply_chunk_size,
                    warn_ms,
                    &cancel_token,
                )
                .await;
                if cancel_token.is_cancelled() {
                    break;
                }
                let any_applied = match result {
                    Ok(any_applied) => any_applied,
                    Err(error) => {
                        consecutive_failures = consecutive_failures.saturating_add(1);
                        crate::sync::log_sync_failure(
                            "Bitcoin RPC sync failed",
                            &error,
                            consecutive_failures,
                        );
                        rpc_client = None;
                        continue;
                    }
                };

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

                if consecutive_failures > 0 {
                    tracing::info!(
                        recovered_after = consecutive_failures,
                        "Bitcoin RPC sync recovered"
                    );
                }
                consecutive_failures = 0;

                cdk_bdk.run_reconciliation().await;
            }
        }
    }
    Ok(())
}

pub(crate) fn classify_bitcoin_rpc_broadcast_message(message: &str) -> BroadcastErrorKind {
    let message = message.to_ascii_lowercase();

    if message.contains("already in block chain")
        || message.contains("already in blockchain")
        || message.contains("already have transaction")
        || message.contains("txn-already-in-mempool")
        || message.contains("transaction already in mempool")
    {
        return BroadcastErrorKind::Unknown;
    }

    if message.contains("missing inputs")
        || message.contains("bad-txns")
        || message.contains("bad-txns-inputs")
        || message.contains("txn-mempool-conflict")
        || message.contains("mandatory-script-verify-flag-failed")
        || message.contains("non-mandatory-script-verify-flag")
        || message.contains("non-bip68-final")
        || message.contains("nonstandard")
        || message.contains("non-standard")
        || message.contains("dust")
        || message.contains("min relay")
        || message.contains("minrelay")
        || message.contains("insufficient fee")
        || message.contains("mempool min fee")
        || message.contains("fee too low")
    {
        return BroadcastErrorKind::Rejected;
    }

    if message.contains("connection")
        || message.contains("timed out")
        || message.contains("timeout")
        || message.contains("broken pipe")
        || message.contains("refused")
        || message.contains("reset")
        || message.contains("temporarily unavailable")
    {
        return BroadcastErrorKind::Transient;
    }

    BroadcastErrorKind::Unknown
}

fn is_bitcoin_rpc_already_known(error: &BitcoinRpcError) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    message.contains("already in block chain")
        || message.contains("already in blockchain")
        || message.contains("already have transaction")
        || message.contains("txn-already-in-mempool")
        || message.contains("transaction already in mempool")
}

pub(crate) fn classify_bitcoin_rpc_broadcast_error(error: &BitcoinRpcError) -> BroadcastErrorKind {
    classify_bitcoin_rpc_broadcast_message(&error.to_string())
}

pub(crate) async fn broadcast_bitcoin_rpc(
    config: &BitcoinRpcConfig,
    tx: Transaction,
) -> Result<BroadcastOutcome, BroadcastFailure> {
    let config = config.clone();
    tokio::task::spawn_blocking(move || {
        let rpc_client: Client = Client::new(
            &format!("http://{}:{}", config.host, config.port),
            Auth::UserPass(config.user, config.password),
        )
        .map_err(|e| BroadcastFailure::new(BroadcastErrorKind::Transient, e.to_string()))?;

        tracing::info!(
            "Broadcasting transaction: {} via bitcoin rpc",
            tx.compute_txid()
        );

        match rpc_client.send_raw_transaction(tx.raw_hex()) {
            Ok(_) => Ok(BroadcastOutcome::Accepted),
            Err(e) if is_bitcoin_rpc_already_known(&e) => Ok(BroadcastOutcome::AlreadyKnown),
            Err(e) => {
                let kind = classify_bitcoin_rpc_broadcast_error(&e);
                Err(BroadcastFailure::new(kind, e.to_string()))
            }
        }
    })
    .await
    .map_err(|error| {
        BroadcastFailure::new(
            BroadcastErrorKind::Transient,
            format!("Bitcoin RPC broadcast task failed: {error}"),
        )
    })?
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

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::str::FromStr;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{mpsc, Mutex as StdMutex};
    use std::thread;

    use bdk_wallet::bitcoin::consensus::encode::serialize_hex;
    use bdk_wallet::bitcoin::{constants::genesis_block, BlockHash, Network, TxOut};
    use bdk_wallet::rusqlite::Connection;
    use bdk_wallet::template::Bip84;
    use bdk_wallet::{KeychainKind, Wallet};
    use serde_json::{json, Value};
    use tokio::sync::oneshot;

    use super::*;

    const TIP_HASH: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const BIRTHDAY_HASH: &str = "2222222222222222222222222222222222222222222222222222222222222222";

    struct TestRpc {
        chain: Vec<Block>,
        stale: Vec<Block>,
        fail_block: Option<BlockHash>,
        fail_header: Option<BlockHash>,
    }

    impl TestRpc {
        fn new(chain: Vec<Block>) -> Self {
            Self {
                chain,
                stale: Vec::new(),
                fail_block: None,
                fail_header: None,
            }
        }
    }

    impl RpcApi for TestRpc {
        fn call<T>(&self, cmd: &str, args: &[Value]) -> Result<T, BitcoinRpcError>
        where
            T: for<'a> serde::Deserialize<'a>,
        {
            let response = match cmd {
                "getblockhash" => {
                    json!(self.chain[args[0].as_u64().expect("height") as usize].block_hash())
                }
                "getblock" | "getblockheader" => {
                    let hash: BlockHash = serde_json::from_value(args[0].clone())?;
                    if (cmd == "getblock" && self.fail_block == Some(hash))
                        || (cmd == "getblockheader" && self.fail_header == Some(hash))
                    {
                        return Err(BitcoinRpcError::ReturnedError(format!(
                            "injected {cmd} failure"
                        )));
                    }
                    let active = self
                        .chain
                        .iter()
                        .position(|block| block.block_hash() == hash);
                    let (height, block) = match active {
                        Some(height) => (height, &self.chain[height]),
                        None => self
                            .stale
                            .iter()
                            .enumerate()
                            .find(|(_, block)| block.block_hash() == hash)
                            .expect("known stale block"),
                    };
                    let verbosity = match cmd {
                        "getblockheader" => 1,
                        _ => args[1].as_u64().expect("verbosity"),
                    };
                    match verbosity {
                        0 => json!(serialize_hex(block)),
                        1 => json!({
                            "hash": hash,
                            "confirmations": if active.is_some() { 1 } else { -1 },
                            "height": height,
                            "size": 0, "weight": 0, "version": 1,
                            "merkleroot": block.header.merkle_root,
                            "tx": [], "time": block.header.time, "nonce": 0,
                            "bits": "207fffff", "difficulty": 1.0,
                            "chainwork": "00", "nTx": block.txdata.len(),
                            "previousblockhash": block.header.prev_blockhash,
                            "nextblockhash": active.and_then(|height| self.chain.get(height + 1))
                                .map(Block::block_hash),
                        }),
                        _ => panic!("unexpected verbosity"),
                    }
                }
                _ => panic!("unexpected RPC {cmd}"),
            };
            serde_json::from_value(response).map_err(BitcoinRpcError::from)
        }
    }

    fn test_wallet() -> Arc<Mutex<WalletWithDb>> {
        let mut db = Connection::open_in_memory().expect("database");
        let key = bdk_wallet::bitcoin::bip32::Xpriv::new_master(Network::Regtest, &[42; 32])
            .expect("master key");
        let wallet = Wallet::create(
            Bip84(key, KeychainKind::External),
            Bip84(key, KeychainKind::Internal),
        )
        .network(Network::Regtest)
        .create_wallet(&mut db)
        .expect("wallet");
        Arc::new(Mutex::new(WalletWithDb::new(wallet, db)))
    }

    fn next_block(previous: &Block, nonce: u32) -> Block {
        let mut block = previous.clone();
        block.header.prev_blockhash = previous.block_hash();
        block.header.nonce = nonce;
        block.txdata.clear();
        block
    }

    fn test_chain(tip: u32) -> Vec<Block> {
        let mut chain = vec![genesis_block(Network::Regtest)];
        for height in 1..=tip {
            chain.push(next_block(chain.last().expect("genesis"), height));
        }
        chain
    }

    async fn wallet_at_checkpoint(block: &Block, height: u32) -> Arc<Mutex<WalletWithDb>> {
        let wallet = test_wallet();
        {
            let mut wallet = wallet.lock().await;
            let checkpoint = wallet.wallet.latest_checkpoint().insert(BlockId {
                height,
                hash: block.block_hash(),
            });
            wallet
                .wallet
                .apply_update(Update {
                    chain: Some(checkpoint),
                    ..Default::default()
                })
                .expect("initial checkpoint");
            wallet.persist().expect("persist checkpoint");
        }
        wallet
    }

    #[tokio::test]
    async fn rpc_sync_recovers_shorter_replacement_chain_below_birthday() {
        let original = test_chain(4);
        let wallet = wallet_at_checkpoint(&original[4], 4).await;
        let replacement_two = next_block(&original[1], 5);
        let mut rpc = TestRpc::new(vec![
            original[0].clone(),
            original[1].clone(),
            replacement_two.clone(),
        ]);
        rpc.fail_block = Some(original[1].block_hash());
        rpc.stale = original;
        let rpc = Arc::new(rpc);
        let cancel = CancellationToken::new();
        assert!(sync_rpc_once(&wallet, rpc.clone(), 1, 500, &cancel)
            .await
            .expect("shorter reorg"));
        assert!(!sync_rpc_once(&wallet, rpc, 1, 500, &cancel)
            .await
            .expect("idle after shorter reorg"));
        let mut wallet = wallet.lock().await;
        let reloaded = Wallet::load()
            .load_wallet(&mut wallet.db)
            .expect("reload")
            .expect("wallet");
        assert_eq!(
            reloaded.latest_checkpoint().block_id(),
            BlockId {
                height: 2,
                hash: replacement_two.block_hash()
            }
        );
    }

    #[tokio::test]
    async fn rpc_sync_reports_missing_reorg_history_and_retries() {
        for missing_header in [true, false] {
            let original = test_chain(3);
            let wallet = wallet_at_checkpoint(&original[3], 3).await;
            let replacement_two = next_block(&original[1], 4);
            let replacement_three = next_block(&replacement_two, 5);
            let replacement = vec![
                original[0].clone(),
                original[1].clone(),
                replacement_two.clone(),
                replacement_three.clone(),
            ];
            let mut rpc = TestRpc::new(replacement.clone());
            rpc.stale = original.clone();
            match missing_header {
                true => {
                    rpc.fail_header = Some(original[2].block_hash());
                    rpc.fail_block = Some(original[1].block_hash());
                }
                false => rpc.fail_block = Some(replacement_two.block_hash()),
            }
            let error = sync_rpc_once(&wallet, Arc::new(rpc), 16, 500, &CancellationToken::new())
                .await
                .expect_err("unavailable recovery history");
            let expected = match missing_header {
                true => "injected getblockheader failure",
                false => "injected getblock failure",
            };
            assert!(
                matches!(error, Error::BitcoinRpc(BitcoinRpcError::ReturnedError(message)) if message == expected)
            );
            assert_eq!(
                wallet.lock().await.wallet.latest_checkpoint().hash(),
                original[3].block_hash()
            );

            let mut rpc = TestRpc::new(replacement);
            rpc.fail_block = Some(original[1].block_hash());
            rpc.stale = original;
            sync_rpc_once(&wallet, Arc::new(rpc), 16, 500, &CancellationToken::new())
                .await
                .expect("recover once required history is available");
            assert_eq!(
                wallet.lock().await.wallet.latest_checkpoint().hash(),
                replacement_three.block_hash()
            );
        }
    }

    struct ReorgDuringFetchRpc {
        before: TestRpc,
        after: TestRpc,
        trigger: BlockHash,
        switched: AtomicBool,
    }

    impl RpcApi for ReorgDuringFetchRpc {
        fn call<T>(&self, cmd: &str, args: &[Value]) -> Result<T, BitcoinRpcError>
        where
            T: for<'a> serde::Deserialize<'a>,
        {
            if self.switched.load(Ordering::SeqCst) {
                return self.after.call(cmd, args);
            }
            let result = self.before.call(cmd, args);
            if cmd == "getblock" && args[0] == json!(self.trigger) {
                self.switched.store(true, Ordering::SeqCst);
            }
            result
        }
    }

    #[tokio::test]
    async fn rpc_sync_retries_a_reorg_during_catch_up_without_skipping_blocks() {
        let original = test_chain(5);
        let wallet = wallet_at_checkpoint(&original[3], 3).await;
        let address = wallet
            .lock()
            .await
            .wallet
            .reveal_next_address(KeychainKind::External);
        let mut payment = original[0].txdata[0].clone();
        payment.output = vec![TxOut {
            value: bdk_wallet::bitcoin::Amount::from_sat(50_000),
            script_pubkey: address.address.script_pubkey(),
        }];
        let mut two = next_block(&original[1], 6);
        two.txdata = vec![payment.clone()];
        two.header.merkle_root = two.compute_merkle_root().expect("merkle root");
        let three = next_block(&two, 7);
        let four = next_block(&three, 8);
        let five = next_block(&four, 9);
        let mut after = TestRpc::new(vec![
            original[0].clone(),
            original[1].clone(),
            two.clone(),
            three,
            four,
            five.clone(),
        ]);
        after.stale = original.clone();
        after.fail_block = Some(original[1].block_hash());
        let rpc = Arc::new(ReorgDuringFetchRpc {
            trigger: original[4].block_hash(),
            before: TestRpc::new(original.clone()),
            after,
            switched: AtomicBool::new(false),
        });
        let cancel = CancellationToken::new();
        assert!(matches!(
            sync_rpc_once(&wallet, rpc.clone(), 16, 500, &cancel).await,
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::Interrupted
        ));
        assert_eq!(
            wallet.lock().await.wallet.latest_checkpoint().hash(),
            original[4].block_hash()
        );
        sync_rpc_once(&wallet, rpc, 16, 500, &cancel)
            .await
            .expect("retry after reorg");
        let mut wallet = wallet.lock().await;
        let reloaded = Wallet::load()
            .load_wallet(&mut wallet.db)
            .expect("reload")
            .expect("wallet");
        assert_eq!(reloaded.latest_checkpoint().hash(), five.block_hash());
        let tx = reloaded
            .get_tx(payment.compute_txid())
            .expect("payment below birthday");
        assert!(matches!(tx.chain_position,
            bdk_wallet::chain::ChainPosition::Confirmed { anchor, .. }
                if anchor.block_id == BlockId { height: 2, hash: two.block_hash() }
        ));
    }

    #[tokio::test]
    async fn rpc_sync_does_not_report_success_for_rollback_without_replacement() {
        let original = test_chain(2);
        let wallet = wallet_at_checkpoint(&original[2], 2).await;
        let mut rpc = TestRpc::new(original[..2].to_vec());
        rpc.stale = original;
        assert!(matches!(
            sync_rpc_once(&wallet, Arc::new(rpc), 16, 500, &CancellationToken::new()).await,
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::Interrupted
        ));
    }

    #[tokio::test]
    async fn rpc_sync_preserves_initial_checkpoint_without_rescanning_history() {
        let wallet = test_wallet();
        let genesis = genesis_block(Network::Regtest);
        let one = next_block(&genesis, 1);
        let two = next_block(&one, 2);
        {
            let mut wallet = wallet.lock().await;
            let checkpoint = wallet.wallet.latest_checkpoint().insert(BlockId {
                height: 2,
                hash: two.block_hash(),
            });
            wallet
                .wallet
                .apply_update(bdk_wallet::Update {
                    chain: Some(checkpoint),
                    ..Default::default()
                })
                .expect("initial checkpoint");
            wallet.persist().expect("persist checkpoint");
        }
        let original = vec![genesis.clone(), one.clone(), two];
        let mut rpc = TestRpc::new(original.clone());
        // An attempt to scan old history fails, as it could on a pruned node.
        rpc.fail_block = Some(one.block_hash());
        assert!(
            !sync_rpc_once(&wallet, Arc::new(rpc), 16, 500, &CancellationToken::new())
                .await
                .expect("already at tip")
        );

        // A one-block reorg removes the only non-genesis checkpoint.
        let replacement_two = next_block(&one, 3);
        let mut rpc = TestRpc::new(vec![genesis, one.clone(), replacement_two.clone()]);
        rpc.stale = original;
        rpc.fail_block = Some(one.block_hash());
        assert!(
            sync_rpc_once(&wallet, Arc::new(rpc), 16, 500, &CancellationToken::new())
                .await
                .expect("shallow reorg without scanning pruned history")
        );
        assert_eq!(
            wallet.lock().await.wallet.latest_checkpoint().hash(),
            replacement_two.block_hash()
        );
    }

    #[tokio::test]
    async fn rpc_sync_replays_reorg_below_birthday_after_reload() {
        let wallet = test_wallet();
        let genesis = genesis_block(Network::Regtest);
        let one = next_block(&genesis, 1);
        let two = next_block(&one, 2);
        let three = next_block(&two, 3);
        let four = next_block(&three, 4);
        let mut previous_chain = vec![genesis.clone(), one.clone(), two, three.clone(), four];
        let address = {
            let mut wallet = wallet.lock().await;
            // The wallet has advanced past its birthday, but has no stored
            // checkpoint at the actual fork point (height 1).
            let checkpoint = wallet
                .wallet
                .latest_checkpoint()
                .insert(BlockId {
                    height: 3,
                    hash: three.block_hash(),
                })
                .insert(BlockId {
                    height: 4,
                    hash: previous_chain[4].block_hash(),
                });
            wallet
                .wallet
                .apply_update(bdk_wallet::Update {
                    chain: Some(checkpoint),
                    ..Default::default()
                })
                .expect("initial checkpoint");
            let address = wallet.wallet.reveal_next_address(KeychainKind::External);
            wallet.persist().expect("persist checkpoint and address");
            address.address
        };
        let cancel = CancellationToken::new();
        sync_rpc_once(
            &wallet,
            Arc::new(TestRpc::new(previous_chain.clone())),
            16,
            500,
            &cancel,
        )
        .await
        .expect("already at tip");

        let mut payment = genesis.txdata[0].clone();
        payment.output = vec![TxOut {
            value: bdk_wallet::bitcoin::Amount::from_sat(50_000),
            script_pubkey: address.script_pubkey(),
        }];
        let mut before_birthday = payment.clone();
        before_birthday.output[0].value = bdk_wallet::bitcoin::Amount::from_sat(25_000);

        // Both payments must be discovered, including the one below the
        // original birthday. Repeat after reloading the persisted checkpoints.
        for nonce in [5, 8] {
            let mut replacement_two = next_block(&one, nonce);
            replacement_two.txdata = vec![before_birthday.clone()];
            replacement_two.header.merkle_root =
                replacement_two.compute_merkle_root().expect("merkle root");
            let mut replacement_three = next_block(&replacement_two, nonce + 1);
            replacement_three.txdata = vec![payment.clone()];
            replacement_three.header.merkle_root = replacement_three
                .compute_merkle_root()
                .expect("merkle root");
            let replacement_four = next_block(&replacement_three, nonce + 2);
            let chain = vec![
                genesis.clone(),
                one.clone(),
                replacement_two.clone(),
                replacement_three.clone(),
                replacement_four.clone(),
            ];
            let mut rpc = TestRpc::new(chain.clone());
            rpc.stale = previous_chain;
            rpc.fail_block = Some(one.block_hash());
            {
                let mut wallet = wallet.lock().await;
                wallet.wallet = Wallet::load()
                    .load_wallet(&mut wallet.db)
                    .expect("reload wallet")
                    .expect("persisted wallet");
            }
            sync_rpc_once(&wallet, Arc::new(rpc), 16, 500, &cancel)
                .await
                .expect("reorg must not scan pruned history");

            let mut wallet = wallet.lock().await;
            let reloaded = Wallet::load()
                .load_wallet(&mut wallet.db)
                .expect("reload after reorg")
                .expect("persisted wallet");
            assert_eq!(
                reloaded.latest_checkpoint().hash(),
                replacement_four.block_hash()
            );
            let tx = reloaded
                .get_tx(before_birthday.compute_txid())
                .expect("payment below original birthday");
            assert!(matches!(tx.chain_position,
                bdk_wallet::chain::ChainPosition::Confirmed { anchor, .. }
                    if anchor.block_id == BlockId { height: 2, hash: replacement_two.block_hash() }
            ));
            let tx = reloaded
                .get_tx(payment.compute_txid())
                .expect("replacement payment");
            assert!(matches!(tx.chain_position,
                bdk_wallet::chain::ChainPosition::Confirmed { anchor, .. }
                    if anchor.block_id == BlockId { height: 3, hash: replacement_three.block_hash() }
            ));
            previous_chain = chain;
        }
    }

    #[tokio::test]
    async fn rpc_sync_replays_payments_below_previous_tip_after_reorg() {
        let wallet = test_wallet();
        let genesis = genesis_block(Network::Regtest);
        let original_one = next_block(&genesis, 1);
        let original_two = next_block(&original_one, 2);
        let original = vec![genesis.clone(), original_one, original_two];
        let cancel = CancellationToken::new();
        sync_rpc_once(
            &wallet,
            Arc::new(TestRpc::new(original.clone())),
            1,
            500,
            &cancel,
        )
        .await
        .expect("initial sync");

        let address = wallet
            .lock()
            .await
            .wallet
            .reveal_next_address(KeychainKind::External);
        let mut payment = genesis.txdata[0].clone();
        payment.output = vec![TxOut {
            value: bdk_wallet::bitcoin::Amount::from_sat(50_000),
            script_pubkey: address.address.script_pubkey(),
        }];
        let mut replacement_one = next_block(&genesis, 3);
        replacement_one.txdata = vec![payment.clone()];
        replacement_one.header.merkle_root =
            replacement_one.compute_merkle_root().expect("merkle root");
        let replacement_two = next_block(&replacement_one, 4);
        let mut rpc = TestRpc::new(vec![
            genesis,
            replacement_one.clone(),
            replacement_two.clone(),
        ]);
        rpc.stale = original;
        sync_rpc_once(&wallet, Arc::new(rpc), 1, 500, &cancel)
            .await
            .expect("reorg sync");

        let mut wallet = wallet.lock().await;
        let reloaded = Wallet::load()
            .load_wallet(&mut wallet.db)
            .expect("reload wallet")
            .expect("persisted wallet");
        assert_eq!(
            reloaded.latest_checkpoint().hash(),
            replacement_two.block_hash()
        );
        let tx = reloaded
            .get_tx(payment.compute_txid())
            .expect("replacement payment discovered");
        assert!(matches!(tx.chain_position,
            bdk_wallet::chain::ChainPosition::Confirmed { anchor, .. }
                if anchor.block_id == BlockId { height: 1, hash: replacement_one.block_hash() }
        ));
    }

    #[tokio::test]
    async fn rpc_sync_retries_failed_persistence_without_new_blocks() {
        let wallet = test_wallet();
        let genesis = genesis_block(Network::Regtest);
        let block = next_block(&genesis, 1);
        let rpc = Arc::new(TestRpc::new(vec![genesis, block.clone()]));
        let cancel = CancellationToken::new();
        wallet
            .lock()
            .await
            .db
            .execute_batch("PRAGMA query_only = ON;")
            .expect("disable writes");

        assert!(matches!(
            sync_rpc_once(&wallet, rpc.clone(), 16, 500, &cancel).await,
            Err(Error::Database(_))
        ));
        assert_eq!(wallet.lock().await.wallet.latest_checkpoint().height(), 1);
        // Even an idle tick must keep reporting the persistence failure.
        assert!(matches!(
            sync_rpc_once(&wallet, rpc.clone(), 16, 500, &cancel).await,
            Err(Error::Database(_))
        ));
        wallet
            .lock()
            .await
            .db
            .execute_batch("PRAGMA query_only = OFF;")
            .expect("enable writes");
        assert!(!sync_rpc_once(&wallet, rpc, 16, 500, &cancel)
            .await
            .expect("idle retry"));

        let mut wallet = wallet.lock().await;
        assert!(wallet.wallet.staged().is_none());
        let reloaded = Wallet::load()
            .load_wallet(&mut wallet.db)
            .expect("reload wallet")
            .expect("persisted wallet");
        assert_eq!(reloaded.latest_checkpoint().hash(), block.block_hash());
    }

    #[tokio::test]
    async fn rpc_sync_persists_partial_chunk_before_fetch_error() {
        let wallet = test_wallet();
        let genesis = genesis_block(Network::Regtest);
        let one = next_block(&genesis, 1);
        let two = next_block(&one, 2);
        let mut rpc = TestRpc::new(vec![genesis, one.clone(), two.clone()]);
        rpc.fail_block = Some(two.block_hash());
        assert!(matches!(
            sync_rpc_once(&wallet, Arc::new(rpc), 16, 500, &CancellationToken::new()).await,
            Err(Error::BitcoinRpc(_))
        ));

        let mut wallet = wallet.lock().await;
        let reloaded = Wallet::load()
            .load_wallet(&mut wallet.db)
            .expect("reload wallet")
            .expect("persisted wallet");
        assert_eq!(reloaded.latest_checkpoint().hash(), one.block_hash());
    }

    struct BlockingRpc {
        inner: TestRpc,
        entered: StdMutex<Option<oneshot::Sender<()>>>,
        release: StdMutex<mpsc::Receiver<()>>,
        fetched: AtomicUsize,
        block_method: &'static str,
    }

    impl RpcApi for BlockingRpc {
        fn call<T>(&self, cmd: &str, args: &[Value]) -> Result<T, BitcoinRpcError>
        where
            T: for<'a> serde::Deserialize<'a>,
        {
            let entered = match cmd == self.block_method {
                true => self.entered.lock().expect("entered lock").take(),
                false => None,
            };
            if let Some(entered) = entered {
                let _ = entered.send(());
                // Bound the wait so a regression blocking the test runtime fails
                // instead of deadlocking the suite.
                let _ = self
                    .release
                    .lock()
                    .expect("release lock")
                    .recv_timeout(Duration::from_secs(2));
            }
            if cmd == "getblock" && args[1] == 0 {
                self.fetched.fetch_add(1, Ordering::SeqCst);
            }
            self.inner.call(cmd, args)
        }
    }

    #[tokio::test]
    async fn rpc_sync_cancellation_during_fetch_leaves_wallet_unchanged() {
        for (drop_future, block_method) in [
            (false, "getblockheader"),
            (true, "getblockheader"),
            (false, "getblock"),
            (true, "getblock"),
        ] {
            let wallet = test_wallet();
            let genesis = genesis_block(Network::Regtest);
            let one = next_block(&genesis, 1);
            let two = next_block(&one, 2);
            let (entered_tx, entered_rx) = oneshot::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let rpc = Arc::new(BlockingRpc {
                inner: TestRpc::new(vec![genesis.clone(), one, two]),
                entered: StdMutex::new(Some(entered_tx)),
                release: StdMutex::new(release_rx),
                fetched: AtomicUsize::new(0),
                block_method,
            });
            let cancel = CancellationToken::new();
            let task_wallet = wallet.clone();
            let task_rpc = rpc.clone();
            let task_cancel = cancel.clone();
            let started = Instant::now();
            let task = tokio::spawn(async move {
                sync_rpc_once(&task_wallet, task_rpc, 16, 500, &task_cancel).await
            });
            entered_rx.await.expect("RPC started");
            // The runtime and wallet must remain available during the RPC.
            let guard = wallet.try_lock().expect("fetch must not hold wallet lock");
            drop(guard);
            match drop_future {
                true => task.abort(),
                false => cancel.cancel(),
            }
            let result = tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .expect("prompt cancellation");
            assert!(
                started.elapsed() < Duration::from_secs(1),
                "RPC blocked the runtime"
            );
            match drop_future {
                true => assert!(result.expect_err("aborted task").is_cancelled()),
                false => assert!(!result.expect("task").expect("sync")),
            }
            release_tx.send(()).expect("release in-flight RPC");
            tokio::time::timeout(Duration::from_secs(1), async {
                while Arc::strong_count(&rpc) > 1 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("blocking worker stopped");
            assert_eq!(
                rpc.fetched.load(Ordering::SeqCst),
                usize::from(block_method == "getblock"),
                "no additional blocks fetched after cancellation"
            );
            assert_eq!(
                wallet.lock().await.wallet.latest_checkpoint().hash(),
                genesis.block_hash()
            );
        }
    }

    fn chain_info(tip: u32) -> Value {
        json!({
            "chain": "regtest",
            "blocks": tip,
            "headers": tip,
            "bestblockhash": TIP_HASH,
            "difficulty": 1.0,
            "mediantime": 0,
            "verificationprogress": 1.0,
            "initialblockdownload": false,
            "chainwork": "00",
            "size_on_disk": 0,
            "pruned": false,
            "warnings": ""
        })
    }

    fn network_info() -> Value {
        json!({ "version": 290000 })
    }

    fn spawn_rpc_server(results: Vec<Value>) -> u16 {
        spawn_rpc_server_with_handler(results, |_| {})
    }

    fn spawn_rpc_server_with_handler<F>(results: Vec<Value>, mut before_response: F) -> u16
    where
        F: FnMut(&Value) + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test RPC server");
        let port = listener.local_addr().expect("test RPC address").port();

        thread::spawn(move || {
            for result in results {
                let (mut stream, _) = listener.accept().expect("accept test RPC connection");
                let mut reader = BufReader::new(&mut stream);
                let mut content_length = None;
                loop {
                    let mut line = String::new();
                    reader
                        .read_line(&mut line)
                        .expect("read test RPC request header");
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(value) = line
                        .to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(str::trim)
                    {
                        content_length =
                            Some(value.parse::<usize>().expect("valid content length"));
                    }
                }
                let mut request_body = vec![0; content_length.expect("request content length")];
                reader
                    .read_exact(&mut request_body)
                    .expect("read test RPC request body");
                let request: Value =
                    serde_json::from_slice(&request_body).expect("valid JSON-RPC request");
                before_response(&request);
                let id = request["id"].clone();
                drop(reader);

                let body = json!({
                    "jsonrpc": "2.0",
                    "result": result,
                    "error": null,
                    "id": id
                })
                .to_string();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .expect("write test RPC response");
                stream.flush().expect("flush test RPC response");
            }
        });

        port
    }

    fn rpc_config(port: u16, wallet_rescan_from_height: Option<u32>) -> BitcoinRpcConfig {
        BitcoinRpcConfig {
            host: "127.0.0.1".to_string(),
            port,
            user: "user".to_string(),
            password: "password".to_string(),
            wallet_rescan_from_height,
        }
    }

    #[test]
    fn fresh_wallet_defaults_to_current_tip() {
        let port = spawn_rpc_server(vec![chain_info(100), network_info()]);

        let checkpoint = initial_checkpoint(&rpc_config(port, None))
            .expect("current tip should become the initial checkpoint");

        assert_eq!(checkpoint.height, 100);
        assert_eq!(
            checkpoint.hash,
            BlockHash::from_str(TIP_HASH).expect("valid tip hash")
        );
    }

    #[test]
    fn fresh_wallet_can_rescan_from_birthday_height() {
        let port = spawn_rpc_server(vec![chain_info(100), network_info(), json!(BIRTHDAY_HASH)]);

        let checkpoint = initial_checkpoint(&rpc_config(port, Some(42)))
            .expect("birthday block should become the initial checkpoint");

        assert_eq!(checkpoint.height, 42);
        assert_eq!(
            checkpoint.hash,
            BlockHash::from_str(BIRTHDAY_HASH).expect("valid birthday hash")
        );
    }

    #[test]
    fn fresh_wallet_rejects_rescan_height_above_tip() {
        let port = spawn_rpc_server(vec![chain_info(100), network_info()]);

        let error = initial_checkpoint(&rpc_config(port, Some(101)))
            .expect_err("future birthday height should fail");

        assert!(matches!(
            error,
            Error::WalletRescanHeightTooHigh {
                requested: 101,
                tip: 100
            }
        ));
    }

    #[test]
    fn classify_bitcoin_rpc_broadcast_errors() {
        assert_eq!(
            classify_bitcoin_rpc_broadcast_message("RPC error: missing inputs"),
            BroadcastErrorKind::Rejected
        );
        assert_eq!(
            classify_bitcoin_rpc_broadcast_message("connection refused"),
            BroadcastErrorKind::Transient
        );
        assert_eq!(
            classify_bitcoin_rpc_broadcast_message("some new backend error"),
            BroadcastErrorKind::Unknown
        );
    }

    #[tokio::test]
    async fn rpc_broadcast_does_not_block_runtime() {
        let tx = genesis_block(Network::Regtest).txdata[0].clone();
        let expected_hex = serialize_hex(&tx);
        let (entered_tx, entered_rx) = oneshot::channel();
        let mut entered_tx = Some(entered_tx);
        let (release_tx, release_rx) = mpsc::channel();
        let port = spawn_rpc_server_with_handler(vec![json!(tx.compute_txid())], move |request| {
            assert_eq!(request["method"], "sendrawtransaction");
            assert_eq!(request["params"][0], expected_hex);
            entered_tx
                .take()
                .expect("single broadcast")
                .send(())
                .expect("notify test");
            // A regression must fail rather than deadlock this single-thread runtime.
            let _ = release_rx.recv_timeout(Duration::from_secs(2));
        });
        let started = Instant::now();
        let task =
            tokio::spawn(async move { broadcast_bitcoin_rpc(&rpc_config(port, None), tx).await });
        entered_rx.await.expect("broadcast started");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "broadcast blocked runtime"
        );
        release_tx.send(()).expect("release RPC");
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .expect("broadcast finished")
                .expect("task")
                .expect("broadcast"),
            BroadcastOutcome::Accepted
        );
    }
}

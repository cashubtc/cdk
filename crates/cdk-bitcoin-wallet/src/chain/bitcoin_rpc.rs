use std::sync::Arc;
use std::time::Instant;

use bdk_bitcoind_rpc::bitcoincore_rpc::{
    jsonrpc, Auth, Client, Error as BitcoinRpcError, RawTx, RpcApi,
};
use bdk_bitcoind_rpc::{BlockEvent, Emitter, NO_EXPECTED_MEMPOOL_TXS};
use bdk_wallet::bitcoin::{Block, Transaction};
use bdk_wallet::chain::BlockId;
use tokio::sync::{mpsc, Mutex};
use tokio::time::Duration;

use crate::chain::{BitcoinRpcConfig, BroadcastErrorKind, BroadcastFailure, BroadcastOutcome};
use crate::error::Error;
use crate::{BdkWalletEngine, WalletWithDb};

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

const RPC_BLOCK_CHANNEL_CAPACITY: usize = 2;

fn build_bitcoin_rpc_client(config: &BitcoinRpcConfig) -> Result<Client, BitcoinRpcError> {
    config
        .validate()
        .map_err(|error| BitcoinRpcError::ReturnedError(error.to_string()))?;
    let url = format!("http://{}:{}", config.host, config.port);
    let transport = jsonrpc::minreq_http::Builder::new()
        .url(&url)
        .map_err(|error| BitcoinRpcError::JsonRpc(error.into()))?
        .basic_auth(config.user.clone(), Some(config.password.clone()))
        .timeout(Duration::from_secs(config.request_timeout_secs))
        .build();
    Ok(Client::from_jsonrpc(jsonrpc::Client::with_transport(
        transport,
    )))
}

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

pub(crate) async fn sync_bitcoin_rpc_once(
    engine: &BdkWalletEngine,
    config: &BitcoinRpcConfig,
) -> Result<(), Error> {
    let checkpoint = {
        let wallet_with_db = engine.wallet_with_db.lock().await;
        wallet_with_db.wallet.latest_checkpoint()
    };
    stream_bitcoin_rpc_blocks(&engine.wallet_with_db, checkpoint, config, 100, u64::MAX)
        .await
        .map(|_| ())
}

async fn stream_bitcoin_rpc_blocks(
    wallet: &std::sync::Arc<Mutex<WalletWithDb>>,
    checkpoint: bdk_wallet::chain::CheckPoint,
    config: &BitcoinRpcConfig,
    apply_chunk_size: usize,
    warn_ms: u64,
) -> Result<bool, Error> {
    let config = config.clone();
    let (sender, mut receiver) = mpsc::channel(RPC_BLOCK_CHANNEL_CAPACITY);
    let producer = tokio::task::spawn_blocking(move || -> Result<(), BitcoinRpcError> {
        let client = build_bitcoin_rpc_client(&config)?;
        // A checkpoint can temporarily be above the new best height during a
        // tip reorg. A zero start height lets `Emitter` walk the checkpoint
        // ancestry to an agreement point instead of querying the now-missing
        // old tip height and failing with "block height out of range".
        let mut emitter = Emitter::new(&client, checkpoint, 0, NO_EXPECTED_MEMPOOL_TXS);
        let mut chunk = Vec::with_capacity(apply_chunk_size);
        while let Some(block) = emitter.next_block()? {
            chunk.push(block);
            if chunk.len() >= apply_chunk_size {
                if sender.blocking_send(std::mem::take(&mut chunk)).is_err() {
                    return Ok(());
                }
                chunk = Vec::with_capacity(apply_chunk_size);
            }
        }
        if !chunk.is_empty() {
            let _ = sender.blocking_send(chunk);
        }
        Ok(())
    });

    let mut applied = false;
    let mut apply_error = None;
    while let Some(mut chunk) = receiver.recv().await {
        match apply_and_persist_chunk(wallet, &mut chunk, warn_ms).await {
            Ok(()) => applied = true,
            Err(error) => {
                apply_error = Some(error);
                receiver.close();
                break;
            }
        }
    }

    let producer_result = producer.await.map_err(|_| {
        Error::BitcoinRpc(BitcoinRpcError::ReturnedError(
            "Bitcoin RPC synchronization worker failed".to_string(),
        ))
    })?;
    if let Some(error) = apply_error {
        return Err(error);
    }
    producer_result?;
    Ok(applied)
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
    tracing::info!(
        "Broadcasting transaction: {} via bitcoin rpc",
        tx.compute_txid()
    );
    let config = config.clone();
    tokio::task::spawn_blocking(move || {
        let rpc_client = build_bitcoin_rpc_client(&config)
            .map_err(|e| BroadcastFailure::new(BroadcastErrorKind::Transient, e.to_string()))?;
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
    .map_err(|_| {
        BroadcastFailure::new(
            BroadcastErrorKind::Transient,
            "Bitcoin RPC broadcast worker failed".to_string(),
        )
    })?
}

pub(crate) async fn fetch_fee_rate_bitcoin_rpc(
    config: &BitcoinRpcConfig,
    target_blocks: u16,
) -> Result<f64, Error> {
    // Use a blocking spawn since Client is synchronous
    let config = config.clone();
    tokio::task::spawn_blocking(move || {
        let rpc_client = build_bitcoin_rpc_client(&config)?;

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
    use std::thread;

    use bdk_wallet::bitcoin::BlockHash;
    use serde_json::{json, Value};

    use super::*;

    const TIP_HASH: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const BIRTHDAY_HASH: &str = "2222222222222222222222222222222222222222222222222222222222222222";

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
            request_timeout_secs: 15,
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

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use bdk_wallet::bitcoin::bip32::{Xpriv, Xpub};
    use bdk_wallet::bitcoin::secp256k1::Secp256k1;
    use bdk_wallet::bitcoin::{absolute, transaction, Network};

    fn start_mock_rpc(
        response: &'static str,
        delay: Duration,
    ) -> (BitcoinRpcConfig, Arc<AtomicUsize>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock Bitcoin RPC");
        let address = listener.local_addr().expect("mock Bitcoin RPC address");
        let requests = Arc::new(AtomicUsize::new(0));
        let request_counter = Arc::clone(&requests);
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept mock RPC request");
            request_counter.fetch_add(1, Ordering::SeqCst);
            let mut request = [0_u8; 16 * 1024];
            let _ = stream.read(&mut request);
            thread::sleep(delay);
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(),
                response
            );
            let _ = stream.write_all(reply.as_bytes());
        });
        (
            BitcoinRpcConfig {
                host: address.ip().to_string(),
                port: address.port(),
                user: "rpc-user".to_string(),
                password: "rpc-password".to_string(),
                request_timeout_secs: 5,
                wallet_rescan_from_height: Some(0),
            },
            requests,
            server,
        )
    }

    fn empty_transaction() -> Transaction {
        Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: Vec::new(),
            output: Vec::new(),
        }
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
    async fn rpc_fee_estimate_and_broadcast_outcomes_are_supported() {
        let (config, _, server) = start_mock_rpc(
            r#"{"result":{"feerate":0.00001,"blocks":1},"error":null,"id":1}"#,
            Duration::ZERO,
        );
        assert_eq!(
            fetch_fee_rate_bitcoin_rpc(&config, 1)
                .await
                .expect("RPC fee estimate"),
            1.0
        );
        server.join().expect("fee server");

        let (config, _, server) = start_mock_rpc(
            r#"{"result":"0000000000000000000000000000000000000000000000000000000000000000","error":null,"id":1}"#,
            Duration::ZERO,
        );
        assert_eq!(
            broadcast_bitcoin_rpc(&config, empty_transaction())
                .await
                .expect("RPC broadcast"),
            BroadcastOutcome::Accepted
        );
        server.join().expect("broadcast server");

        let (config, _, server) = start_mock_rpc(
            r#"{"result":null,"error":{"code":-27,"message":"Transaction already in block chain"},"id":1}"#,
            Duration::ZERO,
        );
        assert_eq!(
            broadcast_bitcoin_rpc(&config, empty_transaction())
                .await
                .expect("already-known RPC broadcast"),
            BroadcastOutcome::AlreadyKnown
        );
        server.join().expect("already-known server");
    }

    #[tokio::test]
    async fn rpc_malformed_and_timed_out_responses_fail_closed() {
        let (config, _, server) = start_mock_rpc("not-json", Duration::ZERO);
        assert!(fetch_fee_rate_bitcoin_rpc(&config, 1).await.is_err());
        server.join().expect("malformed server");

        let (mut config, _, server) = start_mock_rpc(
            r#"{"result":{"feerate":0.00001,"blocks":1},"error":null,"id":1}"#,
            Duration::from_secs(2),
        );
        config.request_timeout_secs = 1;
        assert!(fetch_fee_rate_bitcoin_rpc(&config, 1).await.is_err());
        server.join().expect("timeout server");
    }

    #[tokio::test]
    async fn rpc_network_wait_does_not_hold_the_wallet_lock() {
        let (config, requests, server) = start_mock_rpc("not-json", Duration::from_millis(500));
        let secp = Secp256k1::new();
        let xpriv = Xpriv::new_master(Network::Regtest, &[42; 32]).expect("master key");
        let xpub = Xpub::from_priv(&secp, &xpriv);
        let descriptors = crate::DescriptorConfig::new(
            format!("tr({xpub}/0/*)"),
            format!("tr({xpub}/1/*)"),
            Network::Regtest,
        )
        .expect("descriptors");
        let root = tempfile::tempdir().expect("wallet directory");
        let engine = BdkWalletEngine::open_watch_only(
            descriptors,
            crate::ChainSource::BitcoinRpc(config.clone()),
            root.path(),
        )
        .expect("watch-only wallet");
        let sync_engine = engine.clone();
        let sync = tokio::spawn(async move { sync_bitcoin_rpc_once(&sync_engine, &config).await });

        tokio::time::timeout(Duration::from_secs(1), async {
            while requests.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("network request started");
        let wallet = tokio::time::timeout(Duration::from_millis(100), engine.wallet_with_db.lock())
            .await
            .expect("wallet lock remains available during RPC request");
        drop(wallet);

        assert!(sync.await.expect("sync task").is_err());
        server.join().expect("delayed RPC server");
    }
}

//! Real BIP77 transport and Bitcoin Core validation on an isolated local regtest.

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;

use axum_server::tls_rustls::RustlsConfig;
use bdk_bitcoind_rpc::bitcoincore_rpc::{Auth, Client, RpcApi};
use bdk_wallet::keys::bip39::Mnemonic;
use cdk_common::common::FeeReserve;
use cdk_common::payment::{IncomingPaymentOptions, MintPayment, OnchainIncomingPaymentOptions};
use rustls::pki_types::CertificateDer;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::{BatchConfig, BitcoinRpcConfig, ChainSource};

struct BitcoinNode {
    child: Child,
    port: u16,
    client: Client,
    miner: Client,
    mining_address: Address,
    _temp: tempfile::TempDir,
}

impl Drop for BitcoinNode {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl BitcoinNode {
    async fn start() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let log = std::fs::File::create(temp.path().join("bitcoind.log")).unwrap();
        let child =
            Command::new(std::env::var_os("BITCOIND_EXE").unwrap_or_else(|| "bitcoind".into()))
                .args([
                    "-regtest",
                    "-server",
                    "-listen=0",
                    "-discover=0",
                    "-dnsseed=0",
                    "-fallbackfee=0.00001",
                    "-rpcuser=payjoin-test",
                    "-rpcpassword=payjoin-test",
                    "-rpcbind=127.0.0.1",
                    "-rpcallowip=127.0.0.1",
                ])
                .arg(format!("-datadir={}", temp.path().display()))
                .arg(format!("-rpcport={port}"))
                .stdout(Stdio::from(log.try_clone().unwrap()))
                .stderr(Stdio::from(log))
                .spawn()
                .expect("bitcoind must be provided by the Nix regtest shell or BITCOIND_EXE");
        let auth = || Auth::UserPass("payjoin-test".to_string(), "payjoin-test".to_string());
        let client = Client::new(&format!("http://127.0.0.1:{port}"), auth()).unwrap();
        let miner = Client::new(&format!("http://127.0.0.1:{port}/wallet/miner"), auth()).unwrap();
        // Install the cleanup guard before any assertion can fail.
        let mut node = Self {
            child,
            port,
            client,
            miner,
            mining_address: Address::from_str(ADDRESS)
                .unwrap()
                .require_network(Network::Regtest)
                .unwrap(),
            _temp: temp,
        };
        tokio::time::timeout(Duration::from_secs(30), async {
            while node.client.get_block_count().is_err() {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("Bitcoin Core startup");
        node.client
            .create_wallet("miner", None, None, None, None)
            .unwrap();
        node.mining_address = node
            .miner
            .get_new_address(None, None)
            .unwrap()
            .require_network(Network::Regtest)
            .unwrap();
        node.client
            .generate_to_address(101, &node.mining_address)
            .unwrap();
        node
    }
}

struct Services {
    cert: Vec<u8>,
    relay: String,
    directory: String,
    tasks: Vec<tokio::task::AbortHandle>,
    _temp: tempfile::TempDir,
}

impl Drop for Services {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

impl Services {
    async fn start() -> Self {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let temp = tempfile::tempdir().unwrap();
        let key = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let cert = key.cert.der().to_vec();
        let tls = RustlsConfig::from_der(vec![cert.clone()], key.signing_key.serialize_der())
            .await
            .unwrap();
        let mut roots = rustls::RootCertStore::empty();
        roots.add(CertificateDer::from(cert.clone())).unwrap();
        let mut tasks = Vec::new();
        let mut urls = Vec::new();
        for name in ["relay", "directory"] {
            let config = payjoin_mailroom::config::Config::new(
                "127.0.0.1:0".parse().unwrap(),
                temp.path().join(name),
                Duration::from_secs(1),
                None,
            );
            let (port, task) =
                payjoin_mailroom::serve_manual_tls(config, Some(tls.clone()), roots.clone(), None)
                    .await
                    .unwrap();
            urls.push(format!("https://localhost:{port}"));
            tasks.push(task.abort_handle());
        }
        Self {
            cert,
            relay: urls[0].clone(),
            directory: urls[1].clone(),
            tasks,
            _temp: temp,
        }
    }
}

async fn open_backend(
    path: &Path,
    node: &BitcoinNode,
    services: &Services,
    receiver: bool,
) -> CdkBdk {
    let mnemonic = if receiver {
        "legal winner thank year wave sausage worth useful legal winner thank yellow"
    } else {
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
    };
    std::fs::create_dir_all(path).unwrap();
    let db = Arc::new(
        cdk_sqlite::MintSqliteDatabase::new(path.join("sessions.sqlite").to_str().unwrap())
            .await
            .unwrap(),
    );
    let mut backend = CdkBdk::new(
        Mnemonic::from_str(mnemonic).unwrap(),
        Network::Regtest,
        ChainSource::BitcoinRpc(BitcoinRpcConfig {
            host: "127.0.0.1".to_string(),
            port: node.port,
            user: "payjoin-test".to_string(),
            password: "payjoin-test".to_string(),
            wallet_rescan_from_height: Some(0),
        }),
        path.to_str().unwrap().to_string(),
        FeeReserve {
            min_fee_reserve: cdk_common::Amount::new(1, cdk_common::CurrencyUnit::Sat).into(),
            percent_fee_reserve: 0.02,
        },
        db,
        Some(BatchConfig {
            poll_interval: Duration::from_millis(100),
            ..Default::default()
        }),
        1,
        0,
        1,
        1,
        None,
        None,
    )
    .unwrap()
    .with_payjoin(PayjoinConfig {
        relay_url: services.relay.clone(),
        directory_url: receiver.then(|| services.directory.clone()),
    })
    .unwrap();
    backend.payjoin_test_root = Some(services.cert.clone());
    backend
}

struct SyncTask {
    cancel: CancellationToken,
    handle: JoinHandle<Result<(), Error>>,
}

impl SyncTask {
    fn start(backend: &CdkBdk) -> Self {
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let backend = backend.clone();
        Self {
            cancel,
            handle: tokio::spawn(async move { backend.sync_wallet(worker_cancel).await }),
        }
    }
    async fn stop(self) {
        self.cancel.cancel();
        self.handle.await.unwrap().unwrap();
    }
}

async fn wait_for_sync(backend: &CdkBdk, height: u32) {
    tokio::time::timeout(Duration::from_secs(30), async {
        while backend
            .wallet_with_db
            .lock()
            .await
            .wallet
            .latest_checkpoint()
            .height()
            < height
        {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("BDK sync reaches Bitcoin Core tip");
}

async fn fund(node: &BitcoinNode, backend: &CdkBdk, amount: u64) {
    let address = {
        let mut wallet = backend.wallet_with_db.lock().await;
        let address = wallet
            .wallet
            .reveal_next_address(bdk_wallet::KeychainKind::External)
            .address;
        wallet.persist().unwrap();
        address
    };
    node.miner
        .send_to_address(
            &address,
            Amount::from_sat(amount),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires bitcoind; run with --features payjoin-regtest and BITCOIND_EXE"]
async fn live_payjoin_negotiation_survives_restart_and_confirms() {
    run_live(false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires bitcoind; run with --features payjoin-regtest and BITCOIND_EXE"]
async fn live_payjoin_fallback_survives_restart_and_confirms() {
    run_live(true, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires bitcoind; run with --features payjoin-regtest and BITCOIND_EXE"]
async fn live_payjoin_replays_posts_after_cancellation() {
    run_live(false, true).await;
}

async fn run_live(fallback: bool, interrupt_posts: bool) {
    let node = BitcoinNode::start().await;
    let services = Services::start().await;
    let temp = tempfile::tempdir().unwrap();
    let sender_path = temp.path().join("sender");
    let receiver_path = temp.path().join("receiver");
    let mut sender = open_backend(&sender_path, &node, &services, false).await;
    let receiver = open_backend(&receiver_path, &node, &services, true).await;
    let sender_sync = SyncTask::start(&sender);
    let receiver_sync = SyncTask::start(&receiver);
    fund(&node, &sender, 100_000).await;
    fund(&node, &receiver, 50_000).await;
    node.client
        .generate_to_address(1, &node.mining_address)
        .unwrap();
    wait_for_sync(&sender, 102).await;
    wait_for_sync(&receiver, 102).await;
    let quote_id = cdk_common::QuoteId::from_str(&Uuid::new_v4().to_string()).unwrap();
    let incoming = receiver
        .create_incoming_payment_request(IncomingPaymentOptions::Onchain(
            OnchainIncomingPaymentOptions {
                quote_id: quote_id.clone(),
            },
        ))
        .await
        .unwrap();
    let params =
        cdk_common::payjoin::payjoin_v2_from_extra_json(incoming.extra_json.as_ref()).unwrap();
    let intent = intent_at_address(&sender, params, &incoming.request).await;
    // Persist and POST the original over real TLS/OHTTP, then reopen both wallets.
    if interrupt_posts {
        // The initial session insert has four boundaries. The next write saves
        // the POST response, after the directory has accepted the original.
        let cut = super::faults::Cut::new(5);
        super::faults::instrument(&mut sender, cut.clone());
        let worker = sender.clone();
        let worker_intent = intent.clone();
        let task = tokio::spawn(async move { worker.process_payjoin_send(&worker_intent).await });
        tokio::time::timeout(Duration::from_secs(20), cut.reached.notified())
            .await
            .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
    } else {
        sender.process_payjoin_send(&intent).await.unwrap();
    }
    let sender_record =
        Persister::<SessionEvent>::new(sender.storage.clone(), intent.intent_id.to_string());
    let (state, history) = v2::replay_event_log_async(&sender_record).await.unwrap();
    assert!(if interrupt_posts {
        matches!(state, SendSession::WithReplyKey(_))
    } else {
        matches!(state, SendSession::PollingForProposal(_))
    });
    let original = history.fallback_tx();
    drop(sender_record);
    sender_sync.stop().await;
    receiver_sync.stop().await;
    drop(sender);
    drop(receiver);
    let mut sender = open_backend(&sender_path, &node, &services, false).await;
    let mut receiver = open_backend(&receiver_path, &node, &services, true).await;
    let mut sender_sync = SyncTask::start(&sender);
    let mut receiver_sync = SyncTask::start(&receiver);
    let receiver_id = format!("receive_{quote_id}");
    if interrupt_posts {
        use payjoin::receive::v2::{self as receive, ReceiveSession, SessionEvent as ReceiveEvent};

        // Retry with the persisted reply key, original PSBT and reserved inputs.
        sender.process_payjoin_send(&intent).await.unwrap();
        for _ in 0..12 {
            receiver
                .process_payjoin_receiver(&receiver_id)
                .await
                .unwrap();
            let persister =
                Persister::<ReceiveEvent>::new(receiver.storage.clone(), receiver_id.clone());
            if matches!(
                receive::replay_event_log_async(&persister).await.unwrap().0,
                ReceiveSession::PayjoinProposal(_)
            ) {
                break;
            }
        }
        let persister =
            Persister::<ReceiveEvent>::new(receiver.storage.clone(), receiver_id.clone());
        assert!(matches!(
            receive::replay_event_log_async(&persister).await.unwrap().0,
            ReceiveSession::PayjoinProposal(_)
        ));
        drop(persister);
        let cut = super::faults::Cut::new(1);
        super::faults::instrument(&mut receiver, cut.clone());
        let worker = receiver.clone();
        let id = receiver_id.clone();
        let task = tokio::spawn(async move { worker.process_payjoin_receiver(&id).await });
        tokio::time::timeout(Duration::from_secs(20), cut.reached.notified())
            .await
            .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(!receiver
            .storage
            .payjoin_reserved_inputs()
            .await
            .unwrap()
            .is_empty());
        sender_sync.stop().await;
        receiver_sync.stop().await;
        drop(sender);
        drop(receiver);
        sender = open_backend(&sender_path, &node, &services, false).await;
        receiver = open_backend(&receiver_path, &node, &services, true).await;
        sender_sync = SyncTask::start(&sender);
        receiver_sync = SyncTask::start(&receiver);
    }
    if fallback {
        let persister =
            Persister::<SessionEvent>::new(sender.storage.clone(), intent.intent_id.to_string());
        let (SendSession::PollingForProposal(session), _) =
            v2::replay_event_log_async(&persister).await.unwrap()
        else {
            panic!("polling session");
        };
        session.cancel().save_async(&persister).await.unwrap();
    }
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if !fallback {
                receiver
                    .process_payjoin_receiver(&receiver_id)
                    .await
                    .unwrap();
            }
            sender.process_payjoin_sessions().await.unwrap();
            if !node.client.get_raw_mempool().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("negotiation results in a real mempool transaction");
    let txids = node.client.get_raw_mempool().unwrap();
    assert_eq!(txids.len(), 1);
    let confirmed_tx = node.client.get_raw_transaction(&txids[0], None).unwrap();
    if fallback {
        assert_eq!(confirmed_tx, original);
    } else {
        assert_ne!(confirmed_tx.compute_txid(), original.compute_txid());
        assert_eq!(confirmed_tx.input.len(), original.input.len() + 1);
    }
    // Reopen once more after broadcast, before confirming the transaction.
    sender_sync.stop().await;
    receiver_sync.stop().await;
    drop(sender);
    drop(receiver);
    let sender = open_backend(&sender_path, &node, &services, false).await;
    let receiver = open_backend(&receiver_path, &node, &services, true).await;
    sender.recover_send_saga().await.unwrap();
    let sender_sync = SyncTask::start(&sender);
    let receiver_sync = SyncTask::start(&receiver);
    node.client
        .generate_to_address(1, &node.mining_address)
        .unwrap();
    wait_for_sync(&sender, 103).await;
    wait_for_sync(&receiver, 103).await;
    sender.check_send_saga_confirmations().await.unwrap();
    sender.reconcile_payjoin_sends().await.unwrap();
    receiver
        .process_payjoin_receiver(&receiver_id)
        .await
        .unwrap();
    receiver.scan_for_new_payments().await.unwrap();
    receiver.check_receive_saga_confirmations().await.unwrap();
    let receipt = sender
        .storage
        .get_finalized_intent(&intent.intent_id)
        .await
        .unwrap()
        .unwrap();
    assert!(receipt
        .outpoint
        .starts_with(&confirmed_tx.compute_txid().to_string()));
    let receipts = receiver
        .storage
        .get_finalized_receive_intents_by_quote_id(&quote_id.to_string())
        .await
        .unwrap();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].amount_sat, 10_000);
    assert!(sender
        .storage
        .payjoin_reserved_inputs()
        .await
        .unwrap()
        .is_empty());
    assert!(receiver
        .storage
        .payjoin_reserved_inputs()
        .await
        .unwrap()
        .is_empty());
    sender_sync.stop().await;
    receiver_sync.stop().await;
}

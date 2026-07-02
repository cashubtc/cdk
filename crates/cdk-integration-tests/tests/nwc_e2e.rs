//! End-to-end NWC (NIP-47) integration test.
//!
//! This test spins up a local `nostr-rs-relay`, creates a pure test mint with
//! a funded wallet, starts the CDK NWC service backed by a [`WalletNwcHandler`],
//! and uses the `nwc` client crate to exercise the full NIP-47 request/response
//! flow over Nostr:
//!
//! 1. `get_info` — capability advertisement
//! 2. `get_balance` — wallet balance in msat
//! 3. `make_invoice` — create a mint quote (bolt11 invoice)
//! 4. `pay_invoice` — pay a fake invoice via melt
//! 5. `lookup_invoice` — look up a transaction by payment hash
//! 6. `list_transactions` — list wallet transaction history
//!
//! ## Requirements
//!
//! - `nostr-rs-relay` must be on `PATH` (provided by the Nix `regtest` devShell).
//! - `CDK_TEST_DB_TYPE` must be set (e.g. `memory`, `sqlite`).

use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use cdk::wallet::WalletNwcHandler;
use cdk_fake_wallet::create_fake_invoice;
use cdk_integration_tests::init_pure_tests::*;
use cdk_nwc::nip47::{
    ListTransactionsRequest, LookupInvoiceRequest, MakeInvoiceRequest, PayInvoiceRequest,
    TransactionState,
};
use cdk_nwc::{NwcService, NwcServiceConfig};
use nostr_sdk::{Client as NostrClient, Filter, Keys, Kind, PublicKey, RelayUrl, SecretKey};
use nwc::prelude::{NostrWalletConnectOptions, NostrWalletConnectURI, NWC};
use tokio_util::sync::CancellationToken;

/// Manage a local `nostr-rs-relay` subprocess on a free port.
struct NostrRelay {
    child: Option<Child>,
    port: u16,
}

impl NostrRelay {
    /// Start a local `nostr-rs-relay` on a free TCP port.
    ///
    /// Returns `None` if `nostr-rs-relay` is not on `PATH` (e.g. running
    /// outside the Nix devShell), so the test can be skipped.
    fn start() -> Option<Self> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").ok()?;
        let port = listener.local_addr().ok()?.port();
        drop(listener);

        let db_dir = std::env::temp_dir().join(format!("nostr-relay-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&db_dir).ok()?;

        let config_path = db_dir.join("config.toml");
        let config = format!(
            r#"[network]
port = {port}
address = "127.0.0.1"
"#
        );
        std::fs::write(&config_path, config).ok()?;

        let child = Command::new("nostr-rs-relay")
            .arg("--config")
            .arg(&config_path)
            .arg("--db")
            .arg(&db_dir)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .ok()?;

        Some(Self {
            child: Some(child),
            port,
        })
    }

    fn ws_url(&self) -> String {
        format!("ws://127.0.0.1:{}", self.port)
    }
}

impl Drop for NostrRelay {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Poll the relay's TCP port until it accepts connections or the timeout expires.
async fn wait_for_relay(port: u16, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Wait until the service's kind `13194` info event is visible on the relay.
///
/// The service subscribes for requests *before* publishing the info event, so
/// once the info event is fetchable the service is guaranteed to be receiving
/// requests — no arbitrary sleep needed.
async fn wait_for_info_event(
    relay_url: &RelayUrl,
    service_pubkey: PublicKey,
    timeout: Duration,
) -> bool {
    let client = NostrClient::new(Keys::generate());
    if client.add_relay(relay_url.clone()).await.is_err() {
        return false;
    }
    client.connect().await;

    let filter = Filter::new()
        .kind(Kind::WalletConnectInfo)
        .author(service_pubkey);

    let deadline = tokio::time::Instant::now() + timeout;
    let found = loop {
        if tokio::time::Instant::now() >= deadline {
            break false;
        }
        match client
            .fetch_events(filter.clone(), Duration::from_secs(2))
            .await
        {
            Ok(events) if !events.is_empty() => break true,
            _ => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    };

    client.disconnect().await;
    found
}

/// Full end-to-end NWC flow: client → relay → service → wallet → mint.
#[tokio::test]
async fn nwc_e2e_full_flow() {
    let relay = match NostrRelay::start() {
        Some(r) => r,
        None => {
            // In CI a missing relay means a broken devShell — fail loudly
            // instead of silently passing.
            if std::env::var("CI").is_ok() {
                panic!("nostr-rs-relay not on PATH in CI; the regtest devShell must provide it");
            }
            eprintln!("skipping NWC e2e test: nostr-rs-relay not on PATH");
            return;
        }
    };

    assert!(
        wait_for_relay(relay.port, Duration::from_secs(10)).await,
        "nostr-rs-relay did not start"
    );

    setup_tracing();

    let mint = create_and_start_test_mint().await.expect("mint");
    let wallet = create_test_wallet_for_mint(mint.clone())
        .await
        .expect("wallet");

    fund_wallet(wallet.clone(), 1000, None)
        .await
        .expect("fund wallet");

    let wallet = Arc::new(wallet);

    let service_secret_key = wallet.derive_nwc_secret_key().expect("nwc key");
    let service_keys = Keys::parse(&service_secret_key.to_secret_hex()).expect("keys");
    let client_secret = SecretKey::generate();
    let relay_url = RelayUrl::parse(&relay.ws_url()).expect("relay url");

    let service = NwcService::new(NwcServiceConfig {
        service_keys,
        client_secret: client_secret.clone(),
        relays: vec![relay_url.clone()],
        lud16: None,
    })
    .expect("nwc service");

    let connection_uri = service.connection_uri().to_string();
    let service_pubkey = service.service_pubkey();

    let cancel = CancellationToken::new();
    let service_cancel = cancel.clone();
    let handler = Arc::new(WalletNwcHandler::new(wallet.clone(), None));
    let service_task = tokio::spawn(async move {
        if let Err(err) = service.run(handler, service_cancel).await {
            tracing::error!("NWC service stopped: {err}");
        }
    });

    // The service publishes its info event only after its request subscription
    // is live, so seeing the info event means it is ready to serve.
    assert!(
        wait_for_info_event(&relay_url, service_pubkey, Duration::from_secs(10)).await,
        "NWC service did not publish its info event"
    );

    let uri: NostrWalletConnectURI = connection_uri.parse().expect("uri");
    let opts = NostrWalletConnectOptions::new().timeout(Duration::from_secs(30));
    let nwc_client = NWC::with_opts(uri, opts);

    // 1. get_info
    let info = nwc_client.get_info().await.expect("get_info");
    assert_eq!(info.alias.as_deref(), Some("CDK Cashu Wallet"));
    assert!(!info.methods.is_empty());

    // 2. get_balance — 1000 sats = 1,000,000 msat
    let balance = nwc_client.get_balance().await.expect("get_balance");
    assert_eq!(balance, 1_000_000);

    // 3. make_invoice — create a 500 sat mint quote
    let make_invoice_resp = nwc_client
        .make_invoice(MakeInvoiceRequest {
            amount: 500_000,
            description: Some("test invoice".to_string()),
            description_hash: None,
            expiry: None,
        })
        .await
        .expect("make_invoice");
    assert!(!make_invoice_resp.invoice.is_empty());
    let mint_payment_hash = make_invoice_resp.payment_hash.clone();

    // 4. pay_invoice — pay a 100 sat fake invoice via melt
    let fake_invoice = create_fake_invoice(100_000, "test payment".to_string());
    let pay_resp = nwc_client
        .pay_invoice(PayInvoiceRequest {
            id: None,
            invoice: fake_invoice.to_string(),
            amount: None,
        })
    .await
    .expect("pay_invoice");
    // Fake wallet may not return a preimage; the key assertion is that the
    // full NWC round-trip (client → relay → service → wallet → mint) succeeded.
    assert!(pay_resp.fees_paid.is_some());

    // 5. lookup_invoice — the unpaid mint quote should be pending
    let lookup_resp = nwc_client
        .lookup_invoice(LookupInvoiceRequest {
            payment_hash: mint_payment_hash,
            invoice: None,
        })
        .await
        .expect("lookup_invoice");
    assert_eq!(
        lookup_resp.state,
        Some(TransactionState::Pending),
        "unpaid mint quote should be pending"
    );

    // 6. list_transactions — at least the melt (outgoing) transaction
    let transactions = nwc_client
        .list_transactions(ListTransactionsRequest {
            from: None,
            until: None,
            limit: None,
            offset: None,
            unpaid: None,
            transaction_type: None,
        })
        .await
        .expect("list_transactions");
    assert!(
        !transactions.is_empty(),
        "should have at least one transaction"
    );

    nwc_client.shutdown().await;

    // Cancellation alone must stop the service promptly — no abort.
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), service_task)
        .await
        .expect("service should stop on cancellation")
        .expect("service task should not panic");
}

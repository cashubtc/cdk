#![allow(missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use cdk::nuts::CurrencyUnit;
use cdk::wallet::{Wallet, WalletNwcHandler};
use cdk_nwc::{NwcService, NwcServiceConfig};
use cdk_sqlite::wallet::memory;
use nostr_sdk::{Keys, RelayUrl, SecretKey};
use nwc::prelude::{NostrWalletConnectOptions, NostrWalletConnectURI, NWC};
use rand::random;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

const DEFAULT_RELAY: &str = "wss://relay.damus.io";
const DEFAULT_MINT_URL: &str = "https://testnut.cashudevkit.org";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let env_filter = EnvFilter::new("info,nostr_relay_pool=warn,nostr_sdk=warn");
    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    let relay = std::env::var("NWC_RELAY").unwrap_or_else(|_| DEFAULT_RELAY.to_string());
    let mint_url =
        std::env::var("CDK_TEST_MINT_URL").unwrap_or_else(|_| DEFAULT_MINT_URL.to_string());

    let wallet = Arc::new(Wallet::new(
        &mint_url,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await?),
        random::<[u8; 64]>(),
        None,
    )?);

    let service_secret_key = wallet.derive_nwc_secret_key()?.to_secret_hex();
    let service_keys = Keys::parse(&service_secret_key)?;
    let client_secret = SecretKey::generate();
    let relay_url = RelayUrl::parse(&relay)?;

    let service = NwcService::new(NwcServiceConfig {
        service_keys,
        client_secret,
        relays: vec![relay_url],
        lud16: None,
    })?;

    let connection_uri = service.connection_uri().to_string();
    println!("NWC connection URI:");
    println!("{connection_uri}");

    let cancel = CancellationToken::new();
    let service_cancel = cancel.clone();
    let handler = Arc::new(WalletNwcHandler::new(wallet.clone(), None));
    let service_task = tokio::spawn(async move {
        if let Err(err) = service.run(handler, service_cancel).await {
            tracing::error!("NWC service stopped: {err}");
        }
    });

    tokio::time::sleep(Duration::from_secs(2)).await;

    let uri: NostrWalletConnectURI = connection_uri.parse()?;
    let opts = NostrWalletConnectOptions::new().timeout(Duration::from_secs(15));
    let client = NWC::with_opts(uri, opts);

    let info = client.get_info().await?;
    let balance = client.get_balance().await?;

    println!(
        "Wallet service alias: {}",
        info.alias.unwrap_or_else(|| "unknown".to_string())
    );
    println!("Wallet balance: {balance} msat");

    client.shutdown().await;
    cancel.cancel();
    service_task.abort();
    let _ = service_task.await;

    Ok(())
}

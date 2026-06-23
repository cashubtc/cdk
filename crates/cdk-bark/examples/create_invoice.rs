//! Creates a fresh signet `bark` wallet, wraps it in [`BarkMintPayment`],
//! and requests a Lightning mint quote invoice to confirm the backend
//! works end to end against a live Ark server.
//!
//! Run with `cargo run -p cdk-bark --example create_invoice`.
//! Fund the printed Ark address via <https://signet.2nd.dev/> before
//! relying on the wallet for further testing.

use std::sync::Arc;

use bark::lock_manager::memory::MemoryLockManager;
use bark::persist::sqlite::SqliteClient;
use bark::persist::BarkPersister;
use bark::{Config, Wallet};
use bip39::Mnemonic;
use bitcoin::Network;
use cdk_bark::BarkMintPayment;
use cdk_common::amount::Amount;
use cdk_common::common::FeeReserve;
use cdk_common::nuts::CurrencyUnit;
use cdk_common::payment::{Bolt11IncomingPaymentOptions, IncomingPaymentOptions, MintPayment};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let datadir = std::env::temp_dir().join("cdk-bark-example");
    std::fs::create_dir_all(&datadir)?;

    let mnemonic = Mnemonic::generate(12)?;
    println!("Generated mnemonic (testing only, do not reuse): {mnemonic}");

    let config = Config {
        server_address: "https://ark.signet.2nd.dev".into(),
        esplora_address: Some("https://esplora.signet.2nd.dev".into()),
        ..Config::network_default(Network::Signet)
    };

    let db: Arc<dyn BarkPersister> =
        Arc::new(SqliteClient::open(datadir.join("wallet.sqlite"))?);

    let wallet = Wallet::create(
        &mnemonic,
        Network::Signet,
        config,
        db,
        Box::new(MemoryLockManager::new()),
        false,
    )
    .await?;

    let address = wallet.new_address().await?;
    println!("Fund this address via https://signet.2nd.dev/ : {address}");
    println!("Press enter once funded...");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    let backend = BarkMintPayment::new(
        Arc::new(wallet),
        FeeReserve {
            min_fee_reserve: 0.into(),
            percent_fee_reserve: 0.0,
        },
        CurrencyUnit::Sat,
    );

    let response = backend
        .create_incoming_payment_request(IncomingPaymentOptions::Bolt11(
            Bolt11IncomingPaymentOptions {
                amount: Amount::new(1000, CurrencyUnit::Sat),
                description: Some("cdk-bark example invoice".to_string()),
                unix_expiry: None,
            },
        ))
        .await?;

    println!("Invoice: {}", response.request);

    Ok(())
}


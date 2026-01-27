//! Example showing how to mint tokens with BOLT12 using a custom HTTP client
//!
//! This example demonstrates how to:
//! 1. Create a custom HTTP client
//! 2. Create a wallet with custom HTTP transport
//! 3. Request a mint quote using BOLT12
//! 4. Mint tokens from a mint using custom HTTP

use std::str::FromStr;
use std::sync::Arc;

use cdk::error::Error;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::{SendOptions, WalletBuilder};
use cdk::{Amount, StreamExt};
use cdk_common::mint_url::MintUrl;
use cdk_sqlite::wallet::memory;
use rand::random;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let default_filter = "debug";

    let sqlx_filter = "sqlx=warn,hyper_util=warn,reqwest=warn,rustls=warn";

    let env_filter = EnvFilter::new(format!("{},{}", default_filter, sqlx_filter));

    // Parse input
    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    // Initialize the memory store for the wallet
    let localstore = Arc::new(memory::empty().await?);

    // Generate a random seed for the wallet
    let seed = random::<[u8; 64]>();

    // Define the mint URL and currency unit
    let mint_url = "https://fake.thesimplekid.dev";
    let unit = CurrencyUnit::Sat;
    let amount = Amount::from(10);

    let mint_url = MintUrl::from_str(mint_url)?;

    // Create a new wallet
    let wallet = WalletBuilder::new()
        .mint_url(mint_url)
        .unit(unit)
        .localstore(localstore)
        .seed(seed)
        .target_proof_count(3)
        .build()?;

    let quotes = vec![
        wallet.mint_bolt12_quote(None, None).await?,
        wallet.mint_bolt12_quote(None, None).await?,
        wallet.mint_bolt12_quote(None, None).await?,
    ];

    let mut stream = wallet.mints_proof_stream(quotes, Default::default(), None);

    let stop = stream.get_cancel_token();

    let mut processed = 0;

    while let Some(proofs) = stream.next().await {
        let (mint_quote, proofs) = proofs?;

        // Mint the received amount
        let receive_amount = proofs.total_amount()?;
        tracing::info!("Received {} from mint {}", receive_amount, mint_quote.id);

        // Send a token with the specified amount
        let prepared_send = wallet.prepare_send(amount, SendOptions::default()).await?;
        let token = prepared_send.confirm(None).await?;
        tracing::info!("Token: {}", token);

        processed += 1;

        if processed == 3 {
            stop.cancel()
        }
    }

    tracing::info!("Stopped the loop after {} quotes being minted", processed);

    Ok(())
}

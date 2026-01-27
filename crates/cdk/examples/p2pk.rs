//! Example showing P2PK (Pay to Public Key) functionality
//!
//! This example demonstrates how to:
//! 1. Create a wallet with P2PK spending conditions
//! 2. Mint tokens
//! 3. Send tokens with P2PK lock
//! 4. Receive tokens with P2PK unlock

use std::sync::Arc;
use std::time::Duration;

use cdk::error::Error;
use cdk::nuts::{CurrencyUnit, SecretKey, SpendingConditions};
use cdk::wallet::{ReceiveOptions, SendOptions, Wallet};
use cdk::Amount;
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
    let amount = Amount::from(100);

    // Create a new wallet
    let wallet = Wallet::new(mint_url, unit, localstore, seed, None)?;

    let quote = wallet.mint_quote(amount, None).await?;
    let proofs = wallet
        .wait_and_mint_quote(
            quote,
            Default::default(),
            Default::default(),
            Duration::from_secs(10),
        )
        .await?;

    // Mint the received amount
    let total_amount = proofs.iter().map(|p| u64::from(p.amount)).sum::<u64>();
    println!("Minted nuts total amount: {}", total_amount);

    // Generate a secret key for spending conditions
    let secret = SecretKey::generate();

    // Create spending conditions using the generated public key
    let spending_conditions = SpendingConditions::new_p2pk(secret.public_key(), None);

    // Get the total balance of the wallet
    let bal = wallet.total_balance().await?;
    println!("Total balance: {}", bal);

    let token_amount_to_send = Amount::from(10);

    // Send a token with the specified amount and spending conditions
    let prepared_send = wallet
        .prepare_send(
            token_amount_to_send,
            SendOptions {
                conditions: Some(spending_conditions),
                include_fee: true,
                ..Default::default()
            },
        )
        .await?;

    let swap_fee = prepared_send.swap_fee();

    println!("Fee: {}", swap_fee);

    let token = prepared_send.confirm(None).await?;

    println!("Created token locked to pubkey: {}", secret.public_key());
    println!("{}", token);

    // Receive the token using the secret key
    let amount = wallet
        .receive(
            &token.to_string(),
            ReceiveOptions {
                p2pk_signing_keys: vec![secret],
                ..Default::default()
            },
        )
        .await?;

    assert!(amount == token_amount_to_send);

    println!("Redeemed locked token worth: {}", u64::from(amount));

    Ok(())
}

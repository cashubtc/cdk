#![allow(missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use cdk::error::Error;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::mint::MintRequest;
use cdk::wallet::send::SendRequest;
use cdk::wallet::Wallet;
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
    let mint_url = "https://testnut.cashudevkit.org";
    let unit = CurrencyUnit::Sat;
    let amount = Amount::from(10);

    // Create a new wallet
    let wallet = Wallet::open(cdk::wallet::WalletOpenRequest::new(
        cdk::wallet::WalletIdentity::new(mint_url.parse()?, unit),
        localstore,
        seed,
    ))?;

    let session = wallet.request_mint(MintRequest::bolt11(amount)).await?;
    let receipt = session.wait(Duration::from_secs(10)).await?;

    // Mint the received amount
    println!("Received {} from mint {}", receipt.amount, mint_url);

    // Send a token with the specified amount
    let plan = wallet.plan_send(SendRequest::new(amount)).await?;
    let token = plan.execute().await?.token;
    println!("Token:");
    println!("{}", token);

    Ok(())
}

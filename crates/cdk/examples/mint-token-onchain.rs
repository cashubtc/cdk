use std::sync::Arc;

use cdk::error::Error;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::Wallet;
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

    // Create a new wallet
    let wallet = Wallet::new(mint_url, unit, localstore, seed, None)?;

    // Create an onchain mint quote
    let quote = wallet.mint_onchain_quote().await?;

    println!("Onchain mint quote created:");
    println!("Quote ID: {}", quote.id);
    println!("Send funds to address: {}", quote.request);
    println!("Expiry: {:?}", quote.expiry);

    // In a real scenario, you would:
    // 1. Send Bitcoin to the address specified in quote.request
    // 2. Wait for confirmation
    // 3. Then call mint_onchain to claim the minted tokens

    // For this example, we'll simulate waiting and then attempting to mint
    // Note: This will fail in practice unless the mint actually receives funds
    println!("In a real scenario, send Bitcoin to the address above, then run:");
    println!("wallet.mint_onchain(&quote.id, None, Default::default(), None).await");

    // Uncomment the following to attempt minting (will fail without actual payment):
    /*
    let proofs = wallet
        .wait_and_mint_quote(
            quote,
            Default::default(),
            Default::default(),
            Duration::from_secs(10),
        )
        .await?;

    // Mint the received amount
    let receive_amount = proofs.total_amount()?;
    println!("Received {} from mint {}", receive_amount, mint_url);

    // Send a token with the specified amount
    let prepared_send = wallet.prepare_send(amount, SendOptions::default()).await?;
    let token = prepared_send.confirm(None).await?;
    println!("Token:");
    println!("{}", token);
    */

    Ok(())
}

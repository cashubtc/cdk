#![allow(missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use cdk::nuts::CurrencyUnit;
use cdk::wallet::mint::MintRequest;
use cdk::wallet::operation::SyncPolicy;
use cdk::wallet::send::SendRequest;
use cdk::wallet::Wallet;
use cdk::Amount;
use cdk_sqlite::wallet::memory;
use rand::random;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Generate a random seed for the wallet
    let seed = random::<[u8; 64]>();

    // Mint URL and currency unit
    let mint_url = "https://testnut.cashudevkit.org";
    let unit = CurrencyUnit::Sat;
    let amount = Amount::from(10);

    // Initialize the memory store
    let localstore = Arc::new(memory::empty().await?);

    // Create a new wallet
    let wallet = Wallet::open(cdk::wallet::WalletOpenRequest::new(
        cdk::wallet::WalletIdentity::new(mint_url.parse()?, unit),
        localstore,
        seed,
    ))?;

    // Reconcile durable operations, quotes, and proofs after opening.
    let sync = wallet.synchronize(SyncPolicy::Online).await?;
    if sync.recovered_operations + sync.compensated_operations + sync.failed_operations > 0 {
        println!(
            "Recovered {} operations, {} compensated, {} pending, {} failed",
            sync.recovered_operations,
            sync.compensated_operations,
            sync.pending_operations,
            sync.failed_operations
        );
    }

    let session = wallet.request_mint(MintRequest::bolt11(amount)).await?;
    let receipt = session.wait(Duration::from_secs(10)).await?;

    // Mint the received amount
    println!("Minted {}", receipt.amount);

    // Send the token
    let plan = wallet.plan_send(SendRequest::new(amount)).await?;
    let token = plan.execute().await?.token;

    println!("{}", token);

    Ok(())
}

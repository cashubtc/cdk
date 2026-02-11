#![allow(missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{CurrencyUnit, PaymentMethod};
use cdk::wallet::{RecoveryReport, SendOptions, Wallet, WalletTrait};
use cdk::Amount;
use cdk_sqlite::wallet::memory;
use rand::random;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Generate a random seed for the wallet
    let seed = random::<[u8; 64]>();

    // Mint URL and currency unit
    let mint_url = "https://fake.thesimplekid.dev";
    let unit = CurrencyUnit::Sat;
    let amount = Amount::from(10);

    // Initialize the memory store
    let localstore = Arc::new(memory::empty().await?);

    // Create a new wallet
    let wallet = Wallet::new(mint_url, unit, localstore, seed, None)?;

    // Recover from incomplete operations (required after wallet creation)
    let recovery: RecoveryReport = wallet.recover_incomplete_sagas().await?;
    println!(
        "Recovered {} operations, {} compensated, {} skipped, {} failed",
        recovery.recovered, recovery.compensated, recovery.skipped, recovery.failed
    );

    // Check and mint pending mint quotes (optional, requires network)
    let minted = wallet.mint_unissued_quotes().await?;
    if minted > Amount::ZERO {
        println!("Minted {} from pending quotes", minted);
    }

    let quote = wallet
        .mint_quote(PaymentMethod::BOLT11, Some(amount), None, None)
        .await?;
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
    println!("Minted {}", receive_amount);

    // Send the token
    let prepared_send = wallet.prepare_send(amount, SendOptions::default()).await?;
    let token = prepared_send.confirm(None).await?;

    println!("{}", token);

    Ok(())
}

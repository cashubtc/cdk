use std::sync::Arc;
use std::time::Duration;

use cdk::amount::SplitTarget;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{CurrencyUnit, MintQuoteState};
use cdk::wallet::{SendOptions, Wallet};
use cdk::Amount;
use cdk_sqlite::wallet::memory;
use rand::random;
use tokio::time::sleep;

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

    // Request a mint quote from the wallet
    let quote = wallet.mint_quote(amount, None).await?;

    println!("Pay request: {}", quote.request);

    // Check the quote state in a loop with a timeout
    let timeout = Duration::from_secs(60); // Set a timeout duration
    let start = std::time::Instant::now();

    loop {
        let status = wallet.mint_quote_state(&quote.id).await?;

        if status.state == MintQuoteState::Paid {
            break;
        }

        if start.elapsed() >= timeout {
            eprintln!("Timeout while waiting for mint quote to be paid");
            return Err("Timeout while waiting for mint quote to be paid".into());
        }

        println!("Quote state: {}", status.state);

        sleep(Duration::from_secs(5)).await;
    }

    // Mint the received amount
    let proofs = wallet.mint(&quote.id, SplitTarget::default(), None).await?;
    let receive_amount = proofs.total_amount()?;
    println!("Minted {}", receive_amount);

    // Send the token
    let prepared_send = wallet.prepare_send(amount, SendOptions::default()).await?;
    let token = prepared_send.confirm(None).await?;

    println!("{}", token);

    Ok(())
}

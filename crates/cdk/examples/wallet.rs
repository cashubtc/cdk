use std::str::FromStr;
use std::time::Duration;

use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{CurrencyUnit, MintQuoteState};
use cdk::wallet::{MintOptions, SendOptions, WalletBuilder};
use cdk::Amount;
use cdk_common::mint_url::MintUrl;
use rand::Rng;
use tokio::time::sleep;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Generate a random seed for the wallet
    let seed = rand::thread_rng().gen::<[u8; 32]>();

    // Mint URL and currency unit
    let mint_url = "https://testnut.cashu.space";
    let unit = CurrencyUnit::Sat;
    let amount = Amount::from(10);

    // Create a new wallet
    let wallet = WalletBuilder::new(seed.to_vec()).build(MintUrl::from_str(&mint_url)?, unit)?;

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
    let proofs = wallet.mint(&quote.id, MintOptions::default()).await?;
    let receive_amount = proofs.total_amount()?;
    println!("Minted {}", receive_amount);

    // Send the token
    let token = wallet.send(amount, SendOptions::default()).await?;

    println!("{}", token);

    Ok(())
}

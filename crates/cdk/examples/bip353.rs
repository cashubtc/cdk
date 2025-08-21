//! # BIP-353 CDK Example
//!
//! This example demonstrates how to use BIP-353 (Human Readable Bitcoin Payment Instructions)
//! with the CDK wallet. BIP-353 allows users to share simple email-like addresses such as
//! `user@domain.com` instead of complex Bitcoin addresses or Lightning invoices.
//!
//! ## How it works
//!
//! 1. Parse a human-readable address like `alice@example.com`
//! 2. Query DNS TXT records at `alice.user._bitcoin-payment.example.com`
//! 3. Extract Bitcoin URIs from the TXT records
//! 4. Parse payment instructions (Lightning offers, on-chain addresses)
//! 5. Use CDK wallet to execute payments
//!
//! ## Usage
//!
//! ```bash
//! cargo run --example bip353 --features="wallet bip353"
//! ```
//!
//! Note: The example uses a placeholder address that will fail DNS resolution.
//! To test with real addresses, you need a domain with proper BIP-353 DNS records.

use std::sync::Arc;
use std::time::Duration;

use cdk::amount::SplitTarget;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{CurrencyUnit, MintQuoteState};
use cdk::wallet::Wallet;
use cdk::Amount;
use cdk_sqlite::wallet::memory;
use rand::random;
use tokio::time::sleep;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("BIP-353 CDK Example");
    println!("===================");

    // Example BIP-353 address - replace with a real one that has BOLT12 offer
    // For testing, you might need to set up your own DNS records
    let bip353_address = "tsk@thesimplekid.com"; // This is just an example

    println!("Attempting to use BIP-353 address: {}", bip353_address);

    // Generate a random seed for the wallet
    let seed = random::<[u8; 64]>();

    // Mint URL and currency unit
    let mint_url = "https://fake.thesimplekid.dev";
    let unit = CurrencyUnit::Sat;
    let initial_amount = Amount::from(1000); // Start with 1000 sats

    // Initialize the memory store
    let localstore = Arc::new(memory::empty().await?);

    // Create a new wallet
    let wallet = Wallet::new(mint_url, unit, localstore, seed, None)?;

    // First, we need to fund the wallet
    println!("Requesting mint quote for {} sats...", initial_amount);
    let mint_quote = wallet.mint_quote(initial_amount, None).await?;
    println!(
        "Pay this invoice to fund the wallet: {}",
        mint_quote.request
    );

    // In a real application, you would wait for the payment
    // For this example, we'll just demonstrate the BIP353 melt process
    println!("Waiting for payment... (in real use, pay the above invoice)");

    // Check quote state (with timeout for demo purposes)
    let timeout = Duration::from_secs(30);
    let start = std::time::Instant::now();

    while start.elapsed() < timeout {
        let status = wallet.mint_quote_state(&mint_quote.id).await?;

        if status.state == MintQuoteState::Paid {
            break;
        }

        println!("Quote state: {} (waiting...)", status.state);
        sleep(Duration::from_secs(2)).await;
    }

    // Mint the tokens
    let proofs = wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await?;
    let received_amount = proofs.total_amount()?;
    println!("Successfully minted {} sats", received_amount);

    // Now prepare to pay using the BIP353 address
    let payment_amount_sats = 100; // Example: paying 100 sats

    println!(
        "Attempting to pay {} sats using BIP-353 address...",
        payment_amount_sats
    );

    // Use the new wallet method to resolve BIP353 address and get melt quote
    match wallet
        .melt_bip353_quote(bip353_address, payment_amount_sats * 1_000)
        .await
    {
        Ok(melt_quote) => {
            println!("BIP-353 melt quote received:");
            println!("  Quote ID: {}", melt_quote.id);
            println!("  Amount: {} sats", melt_quote.amount);
            println!("  Fee Reserve: {} sats", melt_quote.fee_reserve);
            println!("  State: {}", melt_quote.state);

            // Execute the payment
            match wallet.melt(&melt_quote.id).await {
                Ok(melt_result) => {
                    println!("BIP-353 payment successful!");
                    println!("  State: {}", melt_result.state);
                    println!("  Amount paid: {} sats", melt_result.amount);
                    println!("  Fee paid: {} sats", melt_result.fee_paid);

                    if let Some(preimage) = melt_result.preimage {
                        println!("  Payment preimage: {}", preimage);
                    }
                }
                Err(e) => {
                    println!("BIP-353 payment failed: {}", e);
                }
            }
        }
        Err(e) => {
            println!("Failed to get BIP-353 melt quote: {}", e);
            println!("This could be because:");
            println!("1. The BIP-353 address format is invalid");
            println!("2. DNS resolution failed (expected for this example)");
            println!("3. No Lightning offer found in the DNS records");
            println!("4. DNSSEC validation failed");
        }
    }

    Ok(())
}

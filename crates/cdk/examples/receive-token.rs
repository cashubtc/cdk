#![allow(missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{CurrencyUnit, PaymentMethod};
use cdk::wallet::{ReceiveOptions, SendOptions, Wallet};
use cdk::Amount;
use cdk_sqlite::wallet::memory;
use rand::random;

/// This example demonstrates how to receive a Cashu token.
///
/// It creates two wallets (sender and receiver), mints proofs in the sender wallet,
/// creates a token, and then receives that token in the receiver wallet.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Mint URL and currency unit
    let mint_url = "https://fake.thesimplekid.dev";
    let unit = CurrencyUnit::Sat;
    let amount = Amount::from(10);

    // Create sender wallet
    let sender_seed = random::<[u8; 64]>();
    let sender_store = Arc::new(memory::empty().await?);
    let sender_wallet = Wallet::new(mint_url, unit.clone(), sender_store, sender_seed, None)?;

    // Create receiver wallet (same mint, different seed/store)
    let receiver_seed = random::<[u8; 64]>();
    let receiver_store = Arc::new(memory::empty().await?);
    let receiver_wallet = Wallet::new(mint_url, unit, receiver_store, receiver_seed, None)?;

    // Step 1: Mint proofs in the sender wallet
    println!("Creating mint quote for {} sats...", amount);
    let quote = sender_wallet
        .mint_quote(PaymentMethod::BOLT11, Some(amount), None, None)
        .await?;
    println!("Mint quote created. Invoice: {}", quote.request);

    // Wait for the quote to be paid and mint the proofs
    // Note: With the fake mint, this happens automatically
    let proofs = sender_wallet
        .wait_and_mint_quote(
            quote,
            Default::default(),
            Default::default(),
            Duration::from_secs(30),
        )
        .await?;

    let minted_amount = proofs.total_amount()?;
    println!("Minted {} sats in sender wallet", minted_amount);

    // Step 2: Create a token to send
    println!("\nPreparing to send {} sats...", amount);
    let prepared_send = sender_wallet
        .prepare_send(amount, SendOptions::default())
        .await?;
    let token = prepared_send.confirm(None).await?;
    println!("Token created:\n{}", token);

    // Step 3: Receive the token in the receiver wallet
    println!("\nReceiving token in receiver wallet...");
    let received_amount = receiver_wallet
        .receive(&token.to_string(), ReceiveOptions::default())
        .await?;
    println!("Received {} sats in receiver wallet", received_amount);

    // Verify balances
    let sender_balance = sender_wallet.total_balance().await?;
    let receiver_balance = receiver_wallet.total_balance().await?;
    println!("\nFinal balances:");
    println!("  Sender:   {} sats", sender_balance);
    println!("  Receiver: {} sats", receiver_balance);

    Ok(())
}

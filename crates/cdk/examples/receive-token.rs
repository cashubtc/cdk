#![allow(missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use cdk::nuts::CurrencyUnit;
use cdk::wallet::mint::MintRequest;
use cdk::wallet::receive::ReceiveRequest;
use cdk::wallet::send::SendRequest;
use cdk::wallet::Wallet;
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
    let mint_url = "https://testnut.cashudevkit.org";
    let unit = CurrencyUnit::Sat;
    let amount = Amount::from(10);

    // Create sender wallet
    let sender_seed = random::<[u8; 64]>();
    let sender_store = Arc::new(memory::empty().await?);
    let sender_wallet = Wallet::open(cdk::wallet::WalletOpenRequest::new(
        cdk::wallet::WalletIdentity::new(mint_url.parse()?, unit.clone()),
        sender_store,
        sender_seed,
    ))?;

    // Create receiver wallet (same mint, different seed/store)
    let receiver_seed = random::<[u8; 64]>();
    let receiver_store = Arc::new(memory::empty().await?);
    let receiver_wallet = Wallet::open(cdk::wallet::WalletOpenRequest::new(
        cdk::wallet::WalletIdentity::new(mint_url.parse()?, unit),
        receiver_store,
        receiver_seed,
    ))?;

    // Step 1: Mint proofs in the sender wallet
    println!("Creating mint quote for {} sats...", amount);
    let session = sender_wallet
        .request_mint(MintRequest::bolt11(amount))
        .await?;
    println!(
        "Mint quote created. Invoice: {}",
        session.initial_state().payment_request
    );

    // Wait for the quote to be paid and mint the proofs
    // Note: With the test mint, this happens automatically
    let receipt = session.wait(Duration::from_secs(30)).await?;
    println!("Minted {} sats in sender wallet", receipt.amount);

    // Step 2: Create a token to send
    println!("\nPreparing to send {} sats...", amount);
    let plan = sender_wallet.plan_send(SendRequest::new(amount)).await?;
    let token = plan.execute().await?.token;
    println!("Token created:\n{}", token);

    // Step 3: Receive the token in the receiver wallet
    println!("\nReceiving token in receiver wallet...");
    let received_amount = receiver_wallet
        .receive(ReceiveRequest::new(token.to_string()))
        .await?
        .amount;
    println!("Received {} sats in receiver wallet", received_amount);

    // Verify balances
    let sender_balance = sender_wallet.balance().await?.available;
    let receiver_balance = receiver_wallet.balance().await?.available;
    println!("\nFinal balances:");
    println!("  Sender:   {} sats", sender_balance);
    println!("  Receiver: {} sats", receiver_balance);

    Ok(())
}

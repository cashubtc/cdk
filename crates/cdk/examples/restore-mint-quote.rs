#![allow(missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{CurrencyUnit, PaymentMethod};
use cdk::wallet::Wallet;
use cdk::Amount;
use cdk_sqlite::wallet::memory;
use rand::random;

/// This example demonstrates recovering a mint quote's signing key from a seed.
///
/// It shows:
/// - Creating mint quotes with a wallet, then losing its database before minting
/// - Creating a new wallet with the same seed but fresh storage
/// - Fetching a known quote ID and recovering its NUT-20 signing key
/// - Minting the recovered quote
///
/// Mint quotes are locked to a key derived from the wallet seed (NUT-20), and
/// the mint only issues ecash for a request signed with that key. If the wallet
/// database is lost after paying a quote but before minting it, the funds can
/// still be claimed as long as you have the seed and the quote ID.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configuration
    let mint_url = "https://testnut.cashudevkit.org";
    let unit = CurrencyUnit::Sat;
    let amount = Amount::from(50);

    // Generate a seed - in production, use a mnemonic and store it securely!
    let seed: [u8; 64] = random();

    // ========================================
    // Step 1: Original wallet creates quotes, then loses its database
    // ========================================
    println!("--- ORIGINAL WALLET ---");

    let original_store = Arc::new(memory::empty().await?);
    let original_wallet = Wallet::new(mint_url, unit.clone(), original_store, seed, None)?;

    // Each quote is locked to the next seed-derived key, so the second quote
    // uses key index 1. The test mint pays its quotes automatically.
    original_wallet
        .mint_quote(PaymentMethod::BOLT11, Some(amount), None, None)
        .await?;
    let quote = original_wallet
        .mint_quote(PaymentMethod::BOLT11, Some(amount), None, None)
        .await?;

    // The quote ID is the one thing that must survive, e.g. from a receipt or log.
    let quote_id = quote.id.clone();
    println!("Created quote {quote_id} for {amount} sats");

    // Simulate losing the wallet database before the quote was minted.
    drop(original_wallet);
    println!("Original wallet database lost before minting");

    // ========================================
    // Step 2: Restored wallet recovers the quote
    // ========================================
    println!("\n--- RESTORED WALLET ---");

    let restored_store = Arc::new(memory::empty().await?);
    let restored_wallet = Wallet::new(mint_url, unit, restored_store, seed, None)?;

    // Fetch the quote and search seed-derived keys at indices 0..1000 for the
    // public key the mint reports. A plain `fetch_mint_quote` would store the
    // quote without its signing key, and minting it would fail.
    let quote = restored_wallet
        .fetch_mint_quote_with_key_search(&quote_id, Some(PaymentMethod::BOLT11), 1_000)
        .await?;

    // The search succeeds even when no key matches, so check the result.
    if quote.secret_key.is_none() {
        println!("No signing key found; retry with a larger key search limit");
        return Ok(());
    }
    println!("Recovered signing key for quote {quote_id}");

    // ========================================
    // Step 3: Mint the recovered quote
    // ========================================
    let proofs = restored_wallet
        .wait_and_mint_quote(
            quote,
            Default::default(),
            Default::default(),
            Duration::from_secs(30),
        )
        .await?;

    println!(
        "Minted {} sats from the recovered quote",
        proofs.total_amount()?
    );
    println!(
        "Restored wallet balance: {} sats",
        restored_wallet.total_balance().await?
    );

    Ok(())
}

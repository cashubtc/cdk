//! Example: Polling for NpubCash quotes
//!
//! This example demonstrates:
//! 1. Setting up a wallet with NpubCash integration
//! 2. Subscribing to real-time quote updates via polling
//! 3. Automatically minting ecash when paid quotes are received
//!
//! Environment variables:
//! - NPUBCASH_URL: NpubCash server URL (default: https://npubx.cash)
//! - NOSTR_NSEC: Your Nostr private key (generates new if not provided)
//! - MINT_URL: Mint URL (default: https://mint.minibits.cash/Bitcoin)
//! - AUTO_MINT: Automatically mint paid quotes (default: true)

use std::sync::Arc;

use cdk::amount::{Amount, SplitTarget};
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::Wallet;
use cdk_common::wallet::MintQuote;
use cdk_common::MintQuoteState;
use cdk_sqlite::wallet::memory;
use nostr_sdk::{Keys, ToBech32};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    println!("=== NpubCash Quote Polling Example ===\n");

    // Setup Nostr keys
    let keys = if let Ok(nsec) = std::env::var("NOSTR_NSEC") {
        println!("Using provided Nostr keys");
        Keys::parse(&nsec)?
    } else {
        println!("Generating new Nostr keys");
        let new_keys = Keys::generate();
        println!("Public key (npub): {}", new_keys.public_key().to_bech32()?);
        println!(
            "Private key (save this!): {}\n",
            new_keys.secret_key().to_bech32()?
        );
        new_keys
    };

    // Setup wallet
    let mint_url = std::env::var("MINT_URL")
        .unwrap_or_else(|_| "https://mint.minibits.cash/Bitcoin".to_string());

    println!("Mint URL: {}", mint_url);

    let localstore = memory::empty().await?;
    let seed = keys.secret_key().to_secret_bytes();
    let mut full_seed = [0u8; 64];
    full_seed[..32].copy_from_slice(&seed);

    let wallet = Arc::new(Wallet::new(
        &mint_url,
        CurrencyUnit::Sat,
        Arc::new(localstore),
        full_seed,
        None,
    )?);

    // Enable NpubCash integration
    let npubcash_url =
        std::env::var("NPUBCASH_URL").unwrap_or_else(|_| "https://npubx.cash".to_string());

    println!("NpubCash URL: {}", npubcash_url);
    wallet.enable_npubcash(npubcash_url.clone()).await?;
    println!("✓ NpubCash integration enabled\n");

    // Display the npub.cash address
    let display_url = npubcash_url
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    println!("Your npub.cash address:");
    println!("   {}@{}\n", keys.public_key().to_bech32()?, display_url);
    println!("Send sats to this address to see them appear!\n");

    // Check if auto-minting is enabled
    let auto_mint = std::env::var("AUTO_MINT")
        .map(|v| v != "false")
        .unwrap_or(true);

    if auto_mint {
        println!("Auto-mint is ENABLED - paid quotes will be automatically minted\n");
    } else {
        println!("Auto-mint is DISABLED - quotes will only be added to the database\n");
    }

    println!("Starting quote polling...");
    println!("Press Ctrl+C to stop.\n");

    // Subscribe to quote updates with a callback, handling Ctrl+C
    let wallet_clone = wallet.clone();

    tokio::select! {
        result = wallet.subscribe_npubcash_updates(move |quotes: Vec<MintQuote>| {
            let wallet = wallet_clone.clone();

            println!("Received {} new quote(s)", quotes.len());

            for quote in quotes {
                let amount_str = quote
                    .amount
                    .map_or("unknown".to_string(), |a: Amount| a.to_string());
                println!("  ├─ Quote ID: {}", quote.id);
                println!("  ├─ Amount: {} {}", amount_str, quote.unit);
                println!("  ├─ State: {:?}", quote.state);

                // If the quote is paid and auto-mint is enabled, mint it
                if auto_mint && matches!(quote.state, MintQuoteState::Paid) {
                    println!("  └─ Auto-minting...");

                    let wallet_mint = wallet.clone();
                    let quote_id = quote.id.clone();

                    tokio::spawn(async move {
                        match wallet_mint
                            .mint(&quote_id, SplitTarget::default(), None)
                            .await
                        {
                            Ok(proofs) => match proofs.total_amount() {
                                Ok(amount) => {
                                    println!("     Successfully minted {} sats!", amount);

                                    if let Ok(balance) = wallet_mint.total_balance().await {
                                        println!("     Wallet balance: {} sats", balance);
                                    }
                                }
                                Err(e) => {
                                    println!("     Failed to calculate amount: {}", e);
                                }
                            },
                            Err(e) => {
                                println!("     Failed to mint: {}", e);
                            }
                        }
                    });
                } else {
                    println!("  └─ Added to database");
                }
            }
            println!();
        }) => {
            // Polling returned with an error
            result?;
        }
        _ = tokio::signal::ctrl_c() => {
            println!("\nStopping quote polling...");
        }
    }

    // Show final wallet balance
    let balance = wallet.total_balance().await?;
    println!("Final wallet balance: {} sats\n", balance);

    println!("=== Summary ===");
    println!("This example demonstrated:");
    println!("1. Creating a wallet with NpubCash enabled");
    println!("2. Subscribing to real-time quote updates");
    println!("3. Automatically processing new quotes as they arrive");
    println!("4. Optional auto-minting of paid quotes");
    println!("\nThe subscribe_npubcash_updates() function:");
    println!("  - Polls the NpubCash server for new quotes");
    println!("  - Automatically adds quotes to the wallet database");
    println!("  - Calls your callback with the converted MintQuotes");
    println!("  - Handles all the conversion and storage for you");

    Ok(())
}

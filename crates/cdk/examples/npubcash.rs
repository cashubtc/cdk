//! Example: Requesting and processing a single NpubCash payment
//!
//! This example demonstrates:
//! 1. Setting up a wallet with NpubCash integration
//! 2. Requesting an invoice for a fixed amount via LNURL-pay
//! 3. Waiting for the payment to be processed and ecash to be minted
//!
//! Environment variables:
//! - NOSTR_NSEC: Your Nostr private key (generates new if not provided)
//!
//! Uses constants:
//! - NPUBCASH_URL: https://npubx.cash
//! - MINT_URL: https://fake.thesimplekid.dev (fake mint that auto-pays)
//!
//! This example uses the NpubCash Proof Stream which continuously polls and mints.

use std::sync::Arc;
use std::time::Duration;

use cdk::amount::SplitTarget;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::{Wallet, WalletTrait};
use cdk::StreamExt;
use cdk_sqlite::wallet::memory;
use nostr_sdk::{Keys, ToBech32};

const NPUBCASH_URL: &str = "https://npubx.cash";
const MINT_URL: &str = "https://fake.thesimplekid.dev";
const PAYMENT_AMOUNT_MSATS: u64 = 100000; // 100 sats

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    println!("=== NpubCash Example ===\n");

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
    let mint_url = MINT_URL.to_string();

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
    let npubcash_url = NPUBCASH_URL.to_string();

    println!("NpubCash URL: {}", npubcash_url);
    wallet.enable_npubcash(npubcash_url.clone()).await?;
    println!("✓ NpubCash integration enabled\n");

    // Display the npub.cash address
    let display_url = npubcash_url
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    println!("Your npub.cash address:");
    println!("   {}@{}\n", keys.public_key().to_bech32()?, display_url);
    println!("Requesting invoice for 100 sats from the fake mint...");

    request_invoice(&keys.public_key().to_bech32()?, PAYMENT_AMOUNT_MSATS).await?;

    println!("Invoice requested - the fake mint should auto-pay shortly.\n");

    // Check if auto-minting is enabled
    // Note: With the new stream API, auto-mint is always enabled as it's the primary purpose of the stream.
    println!("Auto-mint is ENABLED - paid quotes will be automatically minted\n");

    println!("Waiting for the invoice to be paid and processed...\n");

    // Subscribe to quote updates and wait for the single payment
    let mut stream =
        wallet.npubcash_proof_stream(SplitTarget::default(), None, Duration::from_secs(5));

    if let Some(result) = stream.next().await {
        match result {
            Ok((quote, proofs)) => {
                let amount_str = quote
                    .amount
                    .map_or("unknown".to_string(), |a| a.to_string());
                println!("Received payment for quote {}", quote.id);
                println!("  ├─ Amount: {} {}", amount_str, quote.unit);

                match proofs.total_amount() {
                    Ok(amount) => {
                        println!("  └─ Successfully minted {} sats!", amount);
                        if let Ok(balance) = wallet.total_balance().await {
                            println!("     New wallet balance: {} sats", balance);
                        }
                    }
                    Err(e) => println!("  └─ Failed to calculate amount: {}", e),
                }
                println!();
            }
            Err(e) => {
                println!("Error processing payment: {}", e);
            }
        }
    } else {
        println!("No payment received within the timeout period.");
    }

    // Show final wallet balance
    let balance = wallet.total_balance().await?;
    println!("Final wallet balance: {} sats\n", balance);

    Ok(())
}

/// Request an invoice via LNURL-pay
async fn request_invoice(npub: &str, amount_msats: u64) -> Result<(), Box<dyn std::error::Error>> {
    let http_client = cdk_common::HttpClient::new();

    let lnurlp_url = format!("{}/.well-known/lnurlp/{}", NPUBCASH_URL, npub);
    let lnurlp_response: serde_json::Value = http_client.fetch(&lnurlp_url).await?;

    let callback = lnurlp_response["callback"]
        .as_str()
        .ok_or("No callback URL")?;

    let invoice_url = format!("{}?amount={}", callback, amount_msats);
    let invoice_response: serde_json::Value = http_client.fetch(&invoice_url).await?;

    let pr = invoice_response["pr"]
        .as_str()
        .ok_or("No payment request")?;
    println!("   Invoice: {}...", &pr[..50.min(pr.len())]);

    Ok(())
}

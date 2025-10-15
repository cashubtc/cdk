//! # Async Melt Example
//!
//! This example demonstrates both synchronous and asynchronous melt operations in CDK.
//!
//! ## What is Melt?
//! Melting is the process of paying a Lightning invoice using ecash tokens. The tokens are
//! "melted" (destroyed) and the Lightning payment is executed.
//!
//! ## Sync vs Async Melt
//!
//! ### Synchronous Melt (Default)
//! - The melt call waits for the Lightning payment to complete
//! - Returns when payment succeeds or fails
//! - Simpler but blocks until completion
//!
//! ### Asynchronous Melt (with Prefer: respond-async header)
//! - The melt call returns immediately with a `Pending` state
//! - You can poll `melt_quote_status()` to check progress
//! - Allows non-blocking payment processing
//! - Useful for long-running payments or UI responsiveness
//!
//! ## Usage
//!
//! Run this example with:
//! ```bash
//! cargo run --example async-melt
//! ```
//!
//! ## What This Example Does
//!
//! 1. Creates a wallet and funds it by minting tokens
//! 2. Performs a synchronous melt (waits for completion)
//! 3. Performs another melt while demonstrating status checking
//! 4. Shows how to poll quote status for async workflows
//!
//! Note: The actual async behavior with `Prefer: respond-async` header is handled
//! internally by the mint. This example shows the wallet-side API for both modes.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use bitcoin::hashes::{sha256, Hash};
use bitcoin::hex::prelude::FromHex;
use bitcoin::secp256k1::Secp256k1;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{CurrencyUnit, MeltQuoteBolt11Response, MeltQuoteState, MeltRequest, SecretKey};
use cdk::wallet::Wallet;
use cdk::Amount;
use cdk_sqlite::wallet::memory;
use lightning_invoice::{Currency, InvoiceBuilder, PaymentSecret};
use rand::Rng;

#[tokio::main]
async fn main() -> Result<()> {
    println!("\n=== Async Melt Example ===\n");
    println!("This example demonstrates both synchronous and asynchronous melt operations.");
    println!("The sync melt waits for payment completion, while async melt returns immediately\n");
    println!("and allows polling for status updates.\n");

    // Initialize the memory store for the wallet
    let localstore = memory::empty().await?;

    // Generate a random seed for the wallet
    let seed = rand::rng().random::<[u8; 64]>();

    // Define the mint URL and currency unit
    let mint_url = "http://127.0.0.1:8085";
    let unit = CurrencyUnit::Sat;
    let mint_amount = Amount::from(50);

    // Create a new wallet
    let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), seed, None)?;

    println!("📝 Step 1: Minting tokens to fund the wallet...");
    let quote = wallet.mint_quote(mint_amount, None).await?;
    let proofs = wallet
        .wait_and_mint_quote(
            quote,
            Default::default(),
            Default::default(),
            Duration::from_secs(10),
        )
        .await?;

    let balance = proofs.total_amount()?;
    println!("✅ Wallet funded with {} sats\n", balance);

    // ========================================
    // SYNCHRONOUS MELT (Default Behavior)
    // ========================================
    println!("=== Synchronous Melt (Default) ===");
    println!("This is the standard behavior - the melt call waits for completion.\n");

    // Create an invoice for the sync melt
    let invoice_amount_msat = 5 * 1000; // 5 sats
    let sync_invoice = create_test_invoice(invoice_amount_msat)?;
    println!("📄 Created invoice for {} sats", invoice_amount_msat / 1000);

    // Get melt quote
    let sync_melt_quote = wallet.melt_quote(sync_invoice.clone(), None).await?;
    println!(
        "💰 Melt quote: {} sats (fee reserve: {} sats)",
        sync_melt_quote.amount, sync_melt_quote.fee_reserve
    );

    // Perform the melt - this will wait for completion
    println!("⏳ Starting synchronous melt...");
    let start = std::time::Instant::now();

    let melted = wallet.melt(&sync_melt_quote.id).await?;

    let elapsed = start.elapsed();
    println!("✅ Sync melt completed in {:?}", elapsed);
    println!(
        "   Amount: {} sats, Fee paid: {} sats, State: {:?}\n",
        melted.amount, melted.fee_paid, melted.state
    );

    // ========================================
    // ASYNCHRONOUS MELT (With Prefer Header)
    // ========================================
    println!("=== Asynchronous Melt (Prefer: respond-async) ===");
    println!("This demonstrates async behavior using the HTTP client directly with the header.\n");

    // Create another invoice for the async melt
    let async_invoice = create_test_invoice(5 * 1000)?;
    println!("📄 Created invoice for {} sats", 5);

    // Get melt quote
    let async_melt_quote = wallet.melt_quote(async_invoice.clone(), None).await?;
    println!(
        "💰 Melt quote: {} sats (fee reserve: {} sats)",
        async_melt_quote.amount, async_melt_quote.fee_reserve
    );

    // Manually create a melt request using reqwest to add the Prefer header
    let proofs = wallet.get_unspent_proofs().await?;
    let melt_request = MeltRequest::new(async_melt_quote.id.clone(), proofs, None);

    // Create HTTP client and construct POST request to melt endpoint
    let client = reqwest::Client::new();
    let melt_url = format!("{}/v1/melt/bolt11", mint_url);

    println!("⏳ Starting async melt with 'Prefer: respond-async' header...");
    let start = std::time::Instant::now();

    let response = client
        .post(&melt_url)
        .header("Prefer", "respond-async")
        .json(&melt_request)
        .send()
        .await?;

    let async_melt_response: MeltQuoteBolt11Response<String> = response.json().await?;

    let initial_elapsed = start.elapsed();
    println!(
        "✅ Async melt initial response received in {:?} - should be fast!",
        initial_elapsed
    );
    println!("   Initial state: {:?}", async_melt_response.state);

    // For async mode, initial response should be pending
    if async_melt_response.state == MeltQuoteState::Pending {
        println!("   ✓ Response is Pending as expected for async mode\n");
    } else {
        println!(
            "   ⚠ Response is {:?} (may have completed immediately)\n",
            async_melt_response.state
        );
    }

    println!("=== Polling for Completion ===");
    println!("Now polling the quote status to wait for final state...\n");

    // Poll for the final state
    let mut final_state = async_melt_response.state;
    let poll_start = std::time::Instant::now();
    let mut poll_count = 0;

    while final_state == MeltQuoteState::Pending && poll_count < 10 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        poll_count += 1;

        println!("   Poll #{}: Checking status...", poll_count);

        let status_response = client
            .get(&format!(
                "{}/v1/melt/quote/bolt11/{}",
                mint_url, async_melt_quote.id
            ))
            .send()
            .await?;

        let status: MeltQuoteBolt11Response<String> = status_response.json().await?;

        final_state = status.state;

        if final_state != MeltQuoteState::Pending {
            println!("   ✓ State changed to: {:?}", final_state);
            break;
        }
    }

    let total_elapsed = start.elapsed();
    println!("\n✅ Async melt final state: {:?}", final_state);
    println!("   Polling took: {:?}", poll_start.elapsed());
    println!("   Total time from initial request: {:?}", total_elapsed);

    println!("\n=== Summary ===");
    println!("✓ Synchronous melts wait for payment completion before returning");
    println!("✓ Asynchronous melts return immediately with Pending state");
    println!("✓ Use melt_quote_status() to poll for completion in async scenarios");
    println!("✓ The CDK wallet handles both modes transparently based on mint support");

    Ok(())
}

/// Helper function to create a test invoice
fn create_test_invoice(amount_msat: u64) -> Result<String> {
    let private_key = SecretKey::from_slice(
        &<[u8; 32]>::from_hex("e126f68f7eafcc8b74f54d269fe206be715000f94dac067d1c04a8ca3b2db734")
            .unwrap(),
    )
    .unwrap();

    let random_bytes = rand::rng().random::<[u8; 32]>();
    let payment_hash = sha256::Hash::from_slice(&random_bytes).unwrap();
    let payment_secret = PaymentSecret([42u8; 32]);

    let invoice = InvoiceBuilder::new(Currency::Bitcoin)
        .amount_milli_satoshis(amount_msat)
        .description("Test payment".into())
        .payment_hash(payment_hash)
        .payment_secret(payment_secret)
        .current_timestamp()
        .min_final_cltv_expiry_delta(144)
        .build_signed(|hash| Secp256k1::new().sign_ecdsa_recoverable(hash, &private_key))
        .unwrap()
        .to_string();

    Ok(invoice)
}

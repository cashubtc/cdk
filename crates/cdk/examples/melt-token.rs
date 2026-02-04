#![allow(missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use bitcoin::hashes::{sha256, Hash};
use bitcoin::hex::prelude::FromHex;
use bitcoin::secp256k1::Secp256k1;
use cdk::error::Error;
use cdk::nuts::{CurrencyUnit, PaymentMethod, SecretKey};
use cdk::wallet::{MeltOutcome, Wallet, WalletTrait};
use cdk::Amount;
use cdk_sqlite::wallet::memory;
use lightning_invoice::{Currency, InvoiceBuilder, PaymentSecret};
use rand::Rng;

#[tokio::main]
async fn main() -> Result<(), Error> {
    // Initialize the memory store for the wallet
    let localstore = memory::empty().await?;

    // Generate a random seed for the wallet
    let seed = rand::rng().random::<[u8; 64]>();

    // Define the mint URL and currency unit
    let mint_url = "https://fake.thesimplekid.dev";
    let unit = CurrencyUnit::Sat;
    let amount = Amount::from(20);

    // Create a new wallet
    let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), seed, None)?;

    // Mint enough tokens for both examples
    let quote = wallet
        .mint_quote(PaymentMethod::BOLT11, Some(amount), None, None)
        .await?;
    let _proofs = wallet
        .wait_and_mint_quote(
            quote,
            Default::default(),
            Default::default(),
            Duration::from_secs(10),
        )
        .await?;

    let balance = wallet.total_balance().await?;
    println!("Minted {} sats from {}", balance, mint_url);

    // Helper to create a test invoice
    let create_test_invoice = |amount_msats: u64, description: &str| {
        let private_key = SecretKey::from_slice(
            &<[u8; 32]>::from_hex(
                "e126f68f7eafcc8b74f54d269fe206be715000f94dac067d1c04a8ca3b2db734",
            )
            .unwrap(),
        )
        .unwrap();
        let random_bytes = rand::rng().random::<[u8; 32]>();
        let payment_hash = sha256::Hash::from_slice(&random_bytes).unwrap();
        let payment_secret = PaymentSecret([42u8; 32]);
        InvoiceBuilder::new(Currency::Bitcoin)
            .amount_milli_satoshis(amount_msats)
            .description(description.into())
            .payment_hash(payment_hash)
            .payment_secret(payment_secret)
            .current_timestamp()
            .min_final_cltv_expiry_delta(144)
            .build_signed(|hash| Secp256k1::new().sign_ecdsa_recoverable(hash, &private_key))
            .unwrap()
            .to_string()
    };

    println!("\n=== Example 1: Synchronous Confirm ===");
    println!("This approach blocks until the payment completes.");
    println!("Use this when you need to wait for completion before continuing.");

    // Create first melt quote
    let invoice1 = create_test_invoice(5 * 1000, "Sync melt example");
    let melt_quote1 = wallet
        .melt_quote(PaymentMethod::BOLT11, invoice1, None, None)
        .await?;
    println!(
        "Melt quote 1: {} sats, fee reserve: {:?}",
        melt_quote1.amount, melt_quote1.fee_reserve
    );

    // Prepare and confirm synchronously
    let prepared1 = wallet
        .prepare_melt(&melt_quote1.id, std::collections::HashMap::new())
        .await?;
    println!(
        "Prepared melt - Amount: {}, Total Fee: {}",
        prepared1.amount(),
        prepared1.total_fee()
    );

    let confirmed1 = prepared1.confirm().await?;
    println!(
        "Sync melt completed: state={:?}, amount={}, fee_paid={}",
        confirmed1.state(),
        confirmed1.amount(),
        confirmed1.fee_paid()
    );

    println!("\n=== Example 2: Async Confirm ===");
    println!(
        "This approach sends the request with async preference and waits for the mint's response."
    );
    println!(
        "If the mint supports async payments, it may return Pending quickly without waiting for"
    );
    println!("the payment to complete. If not, it may block until the payment completes.");

    // Create second melt quote
    let invoice2 = create_test_invoice(5 * 1000, "Async melt example");
    let melt_quote2 = wallet
        .melt_quote(PaymentMethod::BOLT11, invoice2, None, None)
        .await?;
    println!(
        "Melt quote 2: {} sats, fee reserve: {:?}",
        melt_quote2.amount, melt_quote2.fee_reserve
    );

    // Prepare and confirm asynchronously
    let prepared2 = wallet
        .prepare_melt(&melt_quote2.id, std::collections::HashMap::new())
        .await?;
    println!(
        "Prepared melt - Amount: {}, Total Fee: {}",
        prepared2.amount(),
        prepared2.total_fee()
    );

    // confirm_prefer_async waits for the mint's response, which may be quick if async is supported
    let result = prepared2.confirm_prefer_async().await?;

    match result {
        MeltOutcome::Paid(finalized) => {
            println!(
                "Async melt completed immediately: state={:?}, amount={}, fee_paid={}",
                finalized.state(),
                finalized.amount(),
                finalized.fee_paid()
            );
        }
        MeltOutcome::Pending(pending) => {
            println!("Melt is pending, waiting for completion via WebSocket...");
            // You can either await the pending melt directly:

            let finalized = pending.await?;
            println!(
                "Async melt completed after waiting: state={:?}, amount={}, fee_paid={}",
                finalized.state(),
                finalized.amount(),
                finalized.fee_paid()
            );

            // Alternative: Instead of awaiting, you could:
            // 1. Store the quote ID and check status later with:
            //    wallet.check_melt_quote_status(&melt_quote2.id).await?
            // 2. Let the wallet's background task handle it via:
            //    wallet.finalize_pending_melts().await?
        }
    }

    let final_balance = wallet.total_balance().await?;
    println!("\nFinal balance: {} sats", final_balance);

    Ok(())
}

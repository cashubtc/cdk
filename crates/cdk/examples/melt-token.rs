#![allow(missing_docs)]
#![allow(clippy::unwrap_used)]
#![allow(clippy::use_debug)]

use std::sync::Arc;
use std::time::Duration;

use bitcoin::hashes::{sha256, Hash};
use bitcoin::hex::prelude::FromHex;
use bitcoin::secp256k1::Secp256k1;
use cdk::error::Error;
use cdk::nuts::{CurrencyUnit, SecretKey};
use cdk::wallet::mint::MintRequest;
use cdk::wallet::payment::{PaymentConfirmation, PaymentQuoteRequest, PaymentTarget};
use cdk::wallet::Wallet;
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
    let mint_url = "https://testnut.cashudevkit.org";
    let unit = CurrencyUnit::Sat;
    let amount = Amount::from(20);

    // Create a new wallet
    let wallet = Wallet::open(cdk::wallet::WalletOpenRequest::new(
        cdk::wallet::WalletIdentity::new(mint_url.parse()?, unit),
        Arc::new(localstore),
        seed,
    ))?;

    // Mint enough tokens for both examples
    let mint_session = wallet.request_mint(MintRequest::bolt11(amount)).await?;
    mint_session.wait(Duration::from_secs(10)).await?;

    let balance = wallet.balance().await?.available;
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
    let payment_session1 = wallet
        .quote_payment(PaymentQuoteRequest::new(PaymentTarget::bolt11(invoice1)))
        .await?
        .into_single()?;
    let payment_quote1 = payment_session1.quote();
    println!(
        "Payment quote 1: {} sats, fee reserve: {}",
        payment_quote1.amount, payment_quote1.fee_reserve
    );

    let payment_plan1 = payment_session1.prepare().await?;
    println!(
        "Prepared payment - Amount: {}, Maximum Fee: {}",
        payment_plan1.amount(),
        payment_plan1.maximum_fee()
    );

    let receipt1 = payment_plan1.execute().await?;
    println!(
        "Sync payment completed: amount={}, fee_paid={}",
        receipt1.amount, receipt1.fee_paid
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
    let payment_session2 = wallet
        .quote_payment(PaymentQuoteRequest::new(PaymentTarget::bolt11(invoice2)))
        .await?
        .into_single()?;
    let payment_quote2 = payment_session2.quote();
    println!(
        "Payment quote 2: {} sats, fee reserve: {}",
        payment_quote2.amount, payment_quote2.fee_reserve
    );

    let payment_plan2 = payment_session2.prepare().await?;
    println!(
        "Prepared payment - Amount: {}, Maximum Fee: {}",
        payment_plan2.amount(),
        payment_plan2.maximum_fee()
    );

    let result = payment_plan2.submit().await?;

    match result {
        PaymentConfirmation::Completed(receipt) => {
            println!(
                "Async payment completed immediately: amount={}, fee_paid={}",
                receipt.amount, receipt.fee_paid
            );
        }
        PaymentConfirmation::Pending(pending) => {
            println!("Payment is pending, waiting for completion via WebSocket...");
            let receipt = pending.wait().await?;
            println!(
                "Async payment completed after waiting: amount={}, fee_paid={}",
                receipt.amount, receipt.fee_paid
            );

            // Alternative: Instead of awaiting, you could:
            // 1. Persist `pending.operation_id()` and resume it with
            //    `wallet.resume_pending_payment(operation_id)` after a restart.
            // 2. Call `wallet.synchronize(SyncPolicy::Online)` to reconcile every
            //    interrupted operation.
        }
    }

    let final_balance = wallet.balance().await?.available;
    println!("\nFinal balance: {} sats", final_balance);

    Ok(())
}

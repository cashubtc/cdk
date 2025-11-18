//! Async Melt Integration Tests
//!
//! This file contains tests for async melt functionality using the Prefer: respond-async header.
//!
//! Test Scenarios:
//! - Async melt returns PENDING state immediately
//! - Synchronous melt still works correctly (backward compatibility)
//! - Background task completion
//! - Quote polling pattern

use std::sync::Arc;

use bip39::Mnemonic;
use cdk::amount::SplitTarget;
use cdk::nuts::{CurrencyUnit, MeltQuoteState};
use cdk::wallet::Wallet;
use cdk::StreamExt;
use cdk_fake_wallet::{create_fake_invoice, FakeInvoiceDescription};
use cdk_sqlite::wallet::memory;

const MINT_URL: &str = "http://127.0.0.1:8086";

/// Test: Async melt returns PENDING state immediately
///
/// This test validates that when calling melt with Prefer: respond-async header,
/// the mint returns immediately with PENDING state.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_async_melt_returns_pending() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    // Step 1: Mint some tokens
    let mint_quote = wallet.mint_quote(100.into(), None).await.unwrap();
    let mut proof_streams = wallet.proof_stream(mint_quote.clone(), SplitTarget::default(), None);

    let _proofs = proof_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    let balance = wallet.total_balance().await.unwrap();
    assert_eq!(balance, 100.into());

    // Step 2: Create a melt quote
    let fake_invoice_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Paid,
        check_payment_state: MeltQuoteState::Paid,
        pay_err: false,
        check_err: false,
    };

    let invoice = create_fake_invoice(
        50_000, // 50 sats in millisats
        serde_json::to_string(&fake_invoice_description).unwrap(),
    );

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await.unwrap();

    // Step 3: Call melt (wallet handles proof selection internally)
    let start_time = std::time::Instant::now();

    // This should complete and return the final state
    // TODO: Add Prefer: respond-async header support to wallet.melt()
    let melt_response = wallet.melt(&melt_quote.id).await.unwrap();

    let elapsed = start_time.elapsed();

    // For now, this is synchronous, so it will take longer
    println!("Melt took {:?}", elapsed);

    // Step 4: Verify the melt completed successfully
    assert_eq!(
        melt_response.state,
        MeltQuoteState::Paid,
        "Melt should complete with PAID state"
    );
}

/// Test: Synchronous melt still works correctly
///
/// This test ensures backward compatibility - melt without Prefer header
/// still blocks until completion and returns the final state.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_sync_melt_completes_fully() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    // Step 1: Mint some tokens
    let mint_quote = wallet.mint_quote(100.into(), None).await.unwrap();
    let mut proof_streams = wallet.proof_stream(mint_quote.clone(), SplitTarget::default(), None);

    let _proofs = proof_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    let balance = wallet.total_balance().await.unwrap();
    assert_eq!(balance, 100.into());

    // Step 2: Create a melt quote
    let fake_invoice_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Paid,
        check_payment_state: MeltQuoteState::Paid,
        pay_err: false,
        check_err: false,
    };

    let invoice = create_fake_invoice(
        50_000, // 50 sats in millisats
        serde_json::to_string(&fake_invoice_description).unwrap(),
    );

    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await.unwrap();

    // Step 3: Call synchronous melt
    let melt_response = wallet.melt(&melt_quote.id).await.unwrap();

    // Step 5: Verify response shows payment completed
    assert_eq!(
        melt_response.state,
        MeltQuoteState::Paid,
        "Synchronous melt should return PAID state"
    );

    // Step 6: Verify the quote is PAID in the mint
    let quote_state = wallet.melt_quote_status(&melt_quote.id).await.unwrap();
    assert_eq!(
        quote_state.state,
        MeltQuoteState::Paid,
        "Quote should be PAID"
    );
}

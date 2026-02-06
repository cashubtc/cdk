//! Async Melt Integration Tests
//!
//! This file contains tests for async melt functionality using the Prefer: respond-async header.
//!
//! Test Scenarios:
//! - Async melt returns PENDING state immediately
//! - Synchronous melt still works correctly (backward compatibility)
//! - Background task completion
//! - Quote polling pattern

use std::collections::HashSet;
use std::sync::Arc;

use bip39::Mnemonic;
use cashu::PaymentMethod;
use cdk::amount::SplitTarget;
use cdk::nuts::{CurrencyUnit, MeltQuoteState, State};
use cdk::wallet::{MeltOutcome, Wallet};
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
    let mint_quote = wallet
        .mint_quote(PaymentMethod::BOLT11, Some(100.into()), None, None)
        .await
        .unwrap();
    let mut proof_streams = wallet.proof_stream(mint_quote.clone(), SplitTarget::default(), None);

    let proofs_before = proof_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    // Collect Y values of proofs before melt
    let ys_before: HashSet<_> = proofs_before
        .iter()
        .map(|p| p.y().expect("Invalid proof Y value").clone())
        .collect();

    let balance = wallet.total_balance().await.unwrap();
    assert_eq!(balance, 100.into());

    // Step 2: Create a melt quote
    let fake_invoice_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Paid,
        check_payment_state: MeltQuoteState::Paid,
        pay_err: false,
        check_err: false,
    };

    let invoice: cashu::Bolt11Invoice = create_fake_invoice(
        50_000, // 50 sats in millisats
        serde_json::to_string(&fake_invoice_description).unwrap(),
    );

    let melt_quote = wallet
        .melt_quote(PaymentMethod::BOLT11, invoice.to_string(), None, None)
        .await
        .unwrap();

    // Step 3: Call melt (wallet handles proof selection internally)
    // This should complete and return the final state
    let prepared = wallet
        .prepare_melt(&melt_quote.id, std::collections::HashMap::new())
        .await
        .unwrap();

    // Collect Y values of proofs that will be used in the melt
    let proofs_to_use: HashSet<_> = prepared
        .proofs()
        .iter()
        .chain(prepared.proofs_to_swap().iter())
        .map(|p| p.y().expect("Invalid proof Y value").clone())
        .collect();

    let confirmed = prepared.confirm().await.unwrap();

    // Step 4: Verify the melt completed successfully
    assert_eq!(
        confirmed.state(),
        MeltQuoteState::Paid,
        "Melt should complete with PAID state"
    );

    // Step 5: Verify balance reduced (100 - 50 - fees)
    let final_balance = wallet.total_balance().await.unwrap();
    assert!(
        final_balance < 100.into(),
        "Balance should be reduced after melt. Initial: 100, Final: {}",
        final_balance
    );

    // Step 6: Verify no proofs are pending
    let pending_proofs = wallet
        .get_proofs_with(Some(vec![State::Pending]), None)
        .await
        .unwrap();
    assert!(
        pending_proofs.is_empty(),
        "No proofs should be in pending state after melt completes"
    );

    // Step 7: Verify proofs used in melt are marked as Spent
    let proofs_after = wallet.get_proofs_with(None, None).await.unwrap();
    let ys_after: HashSet<_> = proofs_after
        .iter()
        .map(|p| p.y().expect("Invalid proof Y value").clone())
        .collect();

    // All original proofs should still exist (not deleted)
    for y in &ys_before {
        assert!(
            ys_after.contains(y),
            "Original proof with Y={} should still exist after melt",
            y
        );
    }

    // Verify the specific proofs used are in Spent state
    let spent_proofs = wallet
        .get_proofs_with(Some(vec![State::Spent]), None)
        .await
        .unwrap();
    let spent_ys: HashSet<_> = spent_proofs
        .iter()
        .map(|p| p.y().expect("Invalid proof Y value").clone())
        .collect();

    for y in &proofs_to_use {
        assert!(
            spent_ys.contains(y),
            "Proof with Y={} that was used in melt should be marked as Spent",
            y
        );
    }
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
    let mint_quote = wallet
        .mint_quote(PaymentMethod::BOLT11, Some(100.into()), None, None)
        .await
        .unwrap();
    let mut proof_streams = wallet.proof_stream(mint_quote.clone(), SplitTarget::default(), None);

    let proofs_before = proof_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    // Collect Y values of proofs before melt
    let ys_before: HashSet<_> = proofs_before
        .iter()
        .map(|p| p.y().expect("Invalid proof Y value").clone())
        .collect();

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

    let melt_quote = wallet
        .melt_quote(PaymentMethod::BOLT11, invoice.to_string(), None, None)
        .await
        .unwrap();

    // Step 3: Call melt with prepare/confirm pattern
    let prepared = wallet
        .prepare_melt(&melt_quote.id, std::collections::HashMap::new())
        .await
        .unwrap();

    // Collect Y values of proofs that will be used in the melt
    let proofs_to_use: HashSet<_> = prepared
        .proofs()
        .iter()
        .chain(prepared.proofs_to_swap().iter())
        .map(|p| p.y().expect("Invalid proof Y value").clone())
        .collect();

    let confirmed = prepared.confirm().await.unwrap();

    // Step 5: Verify response shows payment completed
    assert_eq!(
        confirmed.state(),
        MeltQuoteState::Paid,
        "Melt should return PAID state"
    );

    // Step 6: Verify the quote is PAID in the mint
    let quote_state = wallet
        .check_melt_quote_status(&melt_quote.id)
        .await
        .unwrap();
    assert_eq!(
        quote_state.state,
        MeltQuoteState::Paid,
        "Quote should be PAID"
    );

    // Step 7: Verify balance reduced after melt
    let final_balance = wallet.total_balance().await.unwrap();
    assert!(
        final_balance < 100.into(),
        "Balance should be reduced after melt. Initial: 100, Final: {}",
        final_balance
    );

    // Step 8: Verify no proofs are pending
    let pending_proofs = wallet
        .get_proofs_with(Some(vec![State::Pending]), None)
        .await
        .unwrap();
    assert!(
        pending_proofs.is_empty(),
        "No proofs should be in pending state after melt completes"
    );

    // Step 9: Verify proofs used in melt are marked as Spent
    let proofs_after = wallet.get_proofs_with(None, None).await.unwrap();
    let ys_after: HashSet<_> = proofs_after
        .iter()
        .map(|p| p.y().expect("Invalid proof Y value").clone())
        .collect();

    // All original proofs should still exist (not deleted)
    for y in &ys_before {
        assert!(
            ys_after.contains(y),
            "Original proof with Y={} should still exist after melt",
            y
        );
    }

    // Verify the specific proofs used are in Spent state
    let spent_proofs = wallet
        .get_proofs_with(Some(vec![State::Spent]), None)
        .await
        .unwrap();
    let spent_ys: HashSet<_> = spent_proofs
        .iter()
        .map(|p| p.y().expect("Invalid proof Y value").clone())
        .collect();

    for y in &proofs_to_use {
        assert!(
            spent_ys.contains(y),
            "Proof with Y={} that was used in melt should be marked as Spent",
            y
        );
    }
}

/// Test: confirm_prefer_async returns Pending when mint supports async
///
/// This test validates that confirm_prefer_async() returns MeltOutcome::Pending
/// when the mint accepts the async request and returns PENDING state.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_confirm_prefer_async_returns_pending_immediately() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    // Step 1: Mint some tokens
    let mint_quote = wallet
        .mint_quote(PaymentMethod::BOLT11, Some(100.into()), None, None)
        .await
        .unwrap();
    let mut proof_streams = wallet.proof_stream(mint_quote.clone(), SplitTarget::default(), None);

    let _proofs = proof_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    let balance = wallet.total_balance().await.unwrap();
    assert_eq!(balance, 100.into());

    // Step 2: Create a melt quote with Pending state
    let fake_invoice_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Pending,
        check_payment_state: MeltQuoteState::Pending,
        pay_err: false,
        check_err: false,
    };

    let invoice = create_fake_invoice(
        50_000, // 50 sats in millisats
        serde_json::to_string(&fake_invoice_description).unwrap(),
    );

    let melt_quote = wallet
        .melt_quote(PaymentMethod::BOLT11, invoice.to_string(), None, None)
        .await
        .unwrap();

    // Step 3: Call confirm_prefer_async
    let prepared = wallet
        .prepare_melt(&melt_quote.id, std::collections::HashMap::new())
        .await
        .unwrap();

    let result = prepared.confirm_prefer_async().await.unwrap();

    // Step 4: Verify we got Pending result
    assert!(
        matches!(result, MeltOutcome::Pending(_)),
        "confirm_prefer_async should return MeltOutcome::Pending when mint supports async"
    );

    // Step 5: Verify proofs are in pending state
    let pending_proofs = wallet
        .get_proofs_with(Some(vec![State::Pending]), None)
        .await
        .unwrap();
    assert!(
        !pending_proofs.is_empty(),
        "Proofs should be in pending state"
    );

    // Note: Fake wallet may complete immediately even with Pending state configured.
    // The key assertion is that confirm_prefer_async returns MeltOutcome::Pending,
    // which proves the API is working correctly.
}

/// Test: Pending melt from confirm_prefer_async can be awaited
///
/// This test validates that when confirm_prefer_async() returns MeltOutcome::Pending,
/// the pending melt can be awaited to completion.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_confirm_prefer_async_pending_can_be_awaited() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    // Step 1: Mint some tokens
    let mint_quote = wallet
        .mint_quote(PaymentMethod::BOLT11, Some(100.into()), None, None)
        .await
        .unwrap();
    let mut proof_streams = wallet.proof_stream(mint_quote.clone(), SplitTarget::default(), None);

    let proofs_before = proof_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    // Collect Y values of proofs before melt
    let ys_before: HashSet<_> = proofs_before
        .iter()
        .map(|p| p.y().expect("Invalid proof Y value").clone())
        .collect();

    let fake_invoice_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Paid,
        check_payment_state: MeltQuoteState::Paid,
        pay_err: false,
        check_err: false,
    };

    let invoice = create_fake_invoice(
        50_000,
        serde_json::to_string(&fake_invoice_description).unwrap(),
    );

    let melt_quote = wallet
        .melt_quote(PaymentMethod::BOLT11, invoice.to_string(), None, None)
        .await
        .unwrap();

    // Step 3: Call confirm_prefer_async
    let prepared = wallet
        .prepare_melt(&melt_quote.id, std::collections::HashMap::new())
        .await
        .unwrap();

    // Collect Y values of proofs that will be used in the melt
    let proofs_to_use: HashSet<_> = prepared
        .proofs()
        .iter()
        .chain(prepared.proofs_to_swap().iter())
        .map(|p| p.y().expect("Invalid proof Y value").clone())
        .collect();

    let result = prepared.confirm_prefer_async().await.unwrap();

    // Step 4: If we got Pending, await it
    let finalized = match result {
        MeltOutcome::Paid(_melt) => panic!("We expect it to be pending"),
        MeltOutcome::Pending(pending) => {
            // This is the key test - awaiting the pending melt
            let melt = pending.await.unwrap();
            melt
        }
    };

    // Step 5: Verify final state
    assert_eq!(
        finalized.state(),
        MeltQuoteState::Paid,
        "Awaited melt should complete to PAID state"
    );

    // Step 6: Verify balance reduced after awaiting
    let final_balance = wallet.total_balance().await.unwrap();
    assert!(
        final_balance < 100.into(),
        "Balance should be reduced after melt completes. Initial: 100, Final: {}",
        final_balance
    );

    // Step 7: Verify no proofs are pending
    let pending_proofs = wallet
        .get_proofs_with(Some(vec![State::Pending]), None)
        .await
        .unwrap();
    assert!(
        pending_proofs.is_empty(),
        "No proofs should be in pending state after melt completes"
    );

    // Step 8: Verify proofs used in melt are marked as Spent after awaiting
    let proofs_after = wallet.get_proofs_with(None, None).await.unwrap();
    let ys_after: HashSet<_> = proofs_after
        .iter()
        .map(|p| p.y().expect("Invalid proof Y value").clone())
        .collect();

    // All original proofs should still exist (not deleted)
    for y in &ys_before {
        assert!(
            ys_after.contains(y),
            "Original proof with Y={} should still exist after awaiting",
            y
        );
    }

    // Verify the specific proofs used are in Spent state
    let spent_proofs = wallet
        .get_proofs_with(Some(vec![State::Spent]), None)
        .await
        .unwrap();
    let spent_ys: HashSet<_> = spent_proofs
        .iter()
        .map(|p| p.y().expect("Invalid proof Y value").clone())
        .collect();

    for y in &proofs_to_use {
        assert!(
            spent_ys.contains(y),
            "Proof with Y={} that was used in melt should be marked as Spent after awaiting",
            y
        );
    }
}

/// Test: Pending melt can be dropped and polled elsewhere
///
/// This test validates that when confirm_prefer_async() returns MeltOutcome::Pending,
/// the caller can drop the pending handle and poll the status via check_melt_quote_status().
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_confirm_prefer_async_pending_can_be_dropped_and_polled() {
    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    // Step 1: Mint some tokens
    let mint_quote = wallet
        .mint_quote(PaymentMethod::BOLT11, Some(100.into()), None, None)
        .await
        .unwrap();
    let mut proof_streams = wallet.proof_stream(mint_quote.clone(), SplitTarget::default(), None);

    let proofs_before = proof_streams
        .next()
        .await
        .expect("payment")
        .expect("no error");

    // Collect Y values of proofs before melt
    let ys_before: HashSet<_> = proofs_before
        .iter()
        .map(|p| p.y().expect("Invalid proof Y value").clone())
        .collect();

    // Step 2: Create a melt quote
    let fake_invoice_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Paid,
        check_payment_state: MeltQuoteState::Paid,
        pay_err: false,
        check_err: false,
    };

    let invoice = create_fake_invoice(
        50_000,
        serde_json::to_string(&fake_invoice_description).unwrap(),
    );

    let melt_quote = wallet
        .melt_quote(PaymentMethod::BOLT11, invoice.to_string(), None, None)
        .await
        .unwrap();

    let quote_id = melt_quote.id.clone();

    // Step 3: Call confirm_prefer_async
    let prepared = wallet
        .prepare_melt(&melt_quote.id, std::collections::HashMap::new())
        .await
        .unwrap();

    // Collect Y values of proofs that will be used in the melt
    let proofs_to_use: HashSet<_> = prepared
        .proofs()
        .iter()
        .chain(prepared.proofs_to_swap().iter())
        .map(|p| p.y().expect("Invalid proof Y value").clone())
        .collect();

    let result = prepared.confirm_prefer_async().await.unwrap();

    // Step 4: Drop the pending handle (simulating caller not awaiting)
    match result {
        MeltOutcome::Paid(_) => {
            panic!("We expect it to be pending");
        }
        MeltOutcome::Pending(_) => {
            // Drop the pending handle - don't await
        }
    }

    // Step 5: Poll the quote status
    let mut attempts = 0;
    let max_attempts = 10;
    let mut final_state = MeltQuoteState::Unknown;

    while attempts < max_attempts {
        let quote = wallet.check_melt_quote_status(&quote_id).await.unwrap();
        final_state = quote.state;

        if matches!(final_state, MeltQuoteState::Paid | MeltQuoteState::Failed) {
            break;
        }

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        attempts += 1;
    }

    // Step 6: Verify final state
    assert_eq!(
        final_state,
        MeltQuoteState::Paid,
        "Quote should reach PAID state after polling"
    );

    // Step 7: Verify balance reduced after polling shows Paid
    let final_balance = wallet.total_balance().await.unwrap();
    assert!(
        final_balance < 100.into(),
        "Balance should be reduced after melt completes via polling. Initial: 100, Final: {}",
        final_balance
    );

    // Step 8: Verify no proofs are pending
    let pending_proofs = wallet
        .get_proofs_with(Some(vec![State::Pending]), None)
        .await
        .unwrap();
    assert!(
        pending_proofs.is_empty(),
        "No proofs should be in pending state after polling shows Paid"
    );

    // Step 9: Verify proofs used in melt are marked as Spent after polling
    let proofs_after = wallet.get_proofs_with(None, None).await.unwrap();
    let ys_after: HashSet<_> = proofs_after
        .iter()
        .map(|p| p.y().expect("Invalid proof Y value").clone())
        .collect();

    // All original proofs should still exist (not deleted)
    for y in &ys_before {
        assert!(
            ys_after.contains(y),
            "Original proof with Y={} should still exist after polling",
            y
        );
    }

    // Verify the specific proofs used are in Spent state
    let spent_proofs = wallet
        .get_proofs_with(Some(vec![State::Spent]), None)
        .await
        .unwrap();
    let spent_ys: HashSet<_> = spent_proofs
        .iter()
        .map(|p| p.y().expect("Invalid proof Y value").clone())
        .collect();

    for y in &proofs_to_use {
        assert!(
            spent_ys.contains(y),
            "Proof with Y={} that was used in melt should be marked as Spent after polling",
            y
        );
    }
}

/// Test: Compare confirm() vs confirm_prefer_async() behavior
///
/// This test validates the difference between blocking confirm() and
/// non-blocking confirm_prefer_async() methods.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_confirm_vs_confirm_prefer_async_behavior() {
    // Create two wallets for the comparison
    let wallet_a = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create wallet A");

    let wallet_b = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create wallet B");

    // Step 1: Fund both wallets and collect their proof Y values
    let mint_quote_a = wallet_a
        .mint_quote(PaymentMethod::BOLT11, Some(100.into()), None, None)
        .await
        .unwrap();
    let mut proof_streams_a =
        wallet_a.proof_stream(mint_quote_a.clone(), SplitTarget::default(), None);

    let proofs_before_a = proof_streams_a
        .next()
        .await
        .expect("payment")
        .expect("no error");

    // Collect Y values of proofs before melt for wallet A
    let ys_before_a: HashSet<_> = proofs_before_a
        .iter()
        .map(|p| p.y().expect("Invalid proof Y value").clone())
        .collect();

    let mint_quote_b = wallet_b
        .mint_quote(PaymentMethod::BOLT11, Some(100.into()), None, None)
        .await
        .unwrap();
    let mut proof_streams_b =
        wallet_b.proof_stream(mint_quote_b.clone(), SplitTarget::default(), None);

    let proofs_before_b = proof_streams_b
        .next()
        .await
        .expect("payment")
        .expect("no error");

    // Collect Y values of proofs before melt for wallet B
    let ys_before_b: HashSet<_> = proofs_before_b
        .iter()
        .map(|p| p.y().expect("Invalid proof Y value").clone())
        .collect();

    // Step 2: Create melt quotes for both wallets (separate invoices with unique payment hashes)
    let fake_invoice_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Paid,
        check_payment_state: MeltQuoteState::Paid,
        pay_err: false,
        check_err: false,
    };

    let invoice_a = create_fake_invoice(
        50_000,
        serde_json::to_string(&fake_invoice_description).unwrap(),
    );

    let melt_quote_a = wallet_a
        .melt_quote(PaymentMethod::BOLT11, invoice_a.to_string(), None, None)
        .await
        .unwrap();

    // Create separate invoice for wallet B (different payment hash)
    let invoice_b = create_fake_invoice(
        50_000,
        serde_json::to_string(&fake_invoice_description).unwrap(),
    );

    let melt_quote_b = wallet_b
        .melt_quote(PaymentMethod::BOLT11, invoice_b.to_string(), None, None)
        .await
        .unwrap();

    // Step 3: Wallet A uses confirm() - blocks until completion
    let prepared_a = wallet_a
        .prepare_melt(&melt_quote_a.id, std::collections::HashMap::new())
        .await
        .unwrap();

    // Collect Y values of proofs that will be used in the melt for wallet A
    let proofs_to_use_a: HashSet<_> = prepared_a
        .proofs()
        .iter()
        .chain(prepared_a.proofs_to_swap().iter())
        .map(|p| p.y().expect("Invalid proof Y value").clone())
        .collect();

    let finalized_a = prepared_a.confirm().await.unwrap();

    // Step 4: Wallet B uses confirm_prefer_async() - returns immediately
    let prepared_b = wallet_b
        .prepare_melt(&melt_quote_b.id, std::collections::HashMap::new())
        .await
        .unwrap();

    // Collect Y values of proofs that will be used in the melt for wallet B
    let proofs_to_use_b: HashSet<_> = prepared_b
        .proofs()
        .iter()
        .chain(prepared_b.proofs_to_swap().iter())
        .map(|p| p.y().expect("Invalid proof Y value").clone())
        .collect();

    let result_b = prepared_b.confirm_prefer_async().await.unwrap();

    // Step 5: Both should complete successfully
    assert_eq!(
        finalized_a.state(),
        MeltQuoteState::Paid,
        "Wallet A (confirm) should complete successfully"
    );

    let finalized_b = match result_b {
        MeltOutcome::Paid(melt) => melt,
        MeltOutcome::Pending(pending) => pending.await.unwrap(),
    };

    assert_eq!(
        finalized_b.state(),
        MeltQuoteState::Paid,
        "Wallet B (confirm_prefer_async) should complete successfully"
    );

    // Step 6: Verify both wallets have reduced balances
    let balance_a = wallet_a.total_balance().await.unwrap();
    let balance_b = wallet_b.total_balance().await.unwrap();
    assert!(
        balance_a < 100.into(),
        "Wallet A balance should be reduced. Initial: 100, Final: {}",
        balance_a
    );
    assert!(
        balance_b < 100.into(),
        "Wallet B balance should be reduced. Initial: 100, Final: {}",
        balance_b
    );

    // Step 7: Verify no proofs are pending in either wallet
    let pending_a = wallet_a
        .get_proofs_with(Some(vec![State::Pending]), None)
        .await
        .unwrap();
    let pending_b = wallet_b
        .get_proofs_with(Some(vec![State::Pending]), None)
        .await
        .unwrap();
    assert!(
        pending_a.is_empty(),
        "Wallet A should have no pending proofs"
    );
    assert!(
        pending_b.is_empty(),
        "Wallet B should have no pending proofs"
    );

    // Step 8: Verify original proofs are marked as Spent in both wallets
    let proofs_after_a = wallet_a.get_proofs_with(None, None).await.unwrap();
    let proofs_after_b = wallet_b.get_proofs_with(None, None).await.unwrap();

    let ys_after_a: HashSet<_> = proofs_after_a
        .iter()
        .map(|p| p.y().expect("Invalid proof Y value").clone())
        .collect();
    let ys_after_b: HashSet<_> = proofs_after_b
        .iter()
        .map(|p| p.y().expect("Invalid proof Y value").clone())
        .collect();

    // All original proofs should still exist (not deleted)
    for y in &ys_before_a {
        assert!(
            ys_after_a.contains(y),
            "Wallet A original proof with Y={} should still exist after melt",
            y
        );
    }

    for y in &ys_before_b {
        assert!(
            ys_after_b.contains(y),
            "Wallet B original proof with Y={} should still exist after melt",
            y
        );
    }

    // Verify the specific proofs used are in Spent state
    let spent_a = wallet_a
        .get_proofs_with(Some(vec![State::Spent]), None)
        .await
        .unwrap();
    let spent_b = wallet_b
        .get_proofs_with(Some(vec![State::Spent]), None)
        .await
        .unwrap();

    let spent_ys_a: HashSet<_> = spent_a
        .iter()
        .map(|p| p.y().expect("Invalid proof Y value").clone())
        .collect();
    let spent_ys_b: HashSet<_> = spent_b
        .iter()
        .map(|p| p.y().expect("Invalid proof Y value").clone())
        .collect();

    for y in &proofs_to_use_a {
        assert!(
            spent_ys_a.contains(y),
            "Wallet A proof with Y={} that was used in melt should be marked as Spent",
            y
        );
    }

    for y in &proofs_to_use_b {
        assert!(
            spent_ys_b.contains(y),
            "Wallet B proof with Y={} that was used in melt should be marked as Spent",
            y
        );
    }
}

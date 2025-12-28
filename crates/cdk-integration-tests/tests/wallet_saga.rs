//! Wallet Saga Integration Tests
//!
//! These tests verify saga-specific behavior that isn't covered by other integration tests:
//! - Proof reservation and isolation
//! - Cancellation/compensation flows
//! - Concurrent saga isolation
//!
//! Basic happy-path flows are covered by other integration tests (fake_wallet.rs,
//! integration_tests_pure.rs, etc.)

use anyhow::Result;
use cashu::MeltQuoteState;
use cdk::nuts::nut00::ProofsMethods;
use cdk::wallet::SendOptions;
use cdk::Amount;
use cdk_fake_wallet::create_fake_invoice;
use cdk_integration_tests::init_pure_tests::*;

// =============================================================================
// Saga-Specific Tests
// =============================================================================

/// Tests that cancelling a prepared send releases proofs back to Unspent
#[tokio::test]
async fn test_send_cancel_releases_proofs() -> Result<()> {
    setup_tracing();
    let mint = create_and_start_test_mint().await?;
    let wallet = create_test_wallet_for_mint(mint.clone()).await?;

    // Fund wallet
    let initial_amount = Amount::from(1000);
    fund_wallet(wallet.clone(), initial_amount.into(), None).await?;

    let send_amount = Amount::from(400);

    // Prepare send
    let prepared = wallet
        .prepare_send(send_amount, SendOptions::default())
        .await?;

    // Verify proofs are reserved
    let reserved_before = wallet.get_reserved_proofs().await?;
    assert!(!reserved_before.is_empty());

    // Cancel the prepared send
    prepared.cancel().await?;

    // Verify proofs are released (no longer reserved)
    let reserved_after = wallet.get_reserved_proofs().await?;
    assert!(reserved_after.is_empty());

    // Verify full balance is restored
    let balance = wallet.total_balance().await?;
    assert_eq!(balance, initial_amount);

    Ok(())
}

/// Tests that proofs reserved by prepare_send cannot be used by another send
#[tokio::test]
async fn test_reserved_proofs_excluded_from_selection() -> Result<()> {
    setup_tracing();
    let mint = create_and_start_test_mint().await?;
    let wallet = create_test_wallet_for_mint(mint.clone()).await?;

    // Fund wallet with exact amount for two sends
    fund_wallet(wallet.clone(), 600, None).await?;

    // First prepare reserves some proofs
    let prepared1 = wallet
        .prepare_send(Amount::from(300), SendOptions::default())
        .await?;

    // Second prepare should still work (different proofs)
    let prepared2 = wallet
        .prepare_send(Amount::from(300), SendOptions::default())
        .await?;

    // Both should have disjoint proofs
    let ys1: std::collections::HashSet<_> = prepared1.proofs().ys()?.into_iter().collect();
    let ys2: std::collections::HashSet<_> = prepared2.proofs().ys()?.into_iter().collect();
    assert!(ys1.is_disjoint(&ys2));

    // Third prepare should fail (all proofs reserved)
    let result = wallet
        .prepare_send(Amount::from(100), SendOptions::default())
        .await;
    assert!(result.is_err());

    // Cancel first, now we should be able to prepare again
    prepared1.cancel().await?;

    let prepared3 = wallet
        .prepare_send(Amount::from(100), SendOptions::default())
        .await;
    assert!(prepared3.is_ok());

    Ok(())
}

/// Tests that multiple concurrent send sagas don't interfere with each other
#[tokio::test]
async fn test_concurrent_sends_isolated() -> Result<()> {
    setup_tracing();
    let mint = create_and_start_test_mint().await?;
    let wallet = create_test_wallet_for_mint(mint.clone()).await?;

    // Fund wallet
    let initial_amount = Amount::from(2000);
    fund_wallet(wallet.clone(), initial_amount.into(), None).await?;

    // Prepare two sends concurrently
    let wallet1 = wallet.clone();
    let wallet2 = wallet.clone();

    let (prepared1, prepared2) = tokio::join!(
        wallet1.prepare_send(Amount::from(300), SendOptions::default()),
        wallet2.prepare_send(Amount::from(400), SendOptions::default())
    );

    let prepared1 = prepared1?;
    let prepared2 = prepared2?;

    // Verify both have reserved proofs (should be different proofs)
    let reserved1 = prepared1.proofs();
    let reserved2 = prepared2.proofs();

    // The proofs should not overlap
    let ys1: std::collections::HashSet<_> = reserved1.ys()?.into_iter().collect();
    let ys2: std::collections::HashSet<_> = reserved2.ys()?.into_iter().collect();
    assert!(ys1.is_disjoint(&ys2));

    // Confirm both
    let (token1, token2) = tokio::join!(prepared1.confirm(None), prepared2.confirm(None));

    let _token1 = token1?;
    let _token2 = token2?;

    // Verify final balance is correct
    let final_balance = wallet.total_balance().await?;
    assert_eq!(final_balance, initial_amount - Amount::from(700));

    Ok(())
}

/// Tests concurrent melt operations are isolated
#[tokio::test]
async fn test_concurrent_melts_isolated() -> Result<()> {
    setup_tracing();
    let mint = create_and_start_test_mint().await?;
    let wallet = create_test_wallet_for_mint(mint.clone()).await?;

    // Fund wallet with enough for multiple melts
    fund_wallet(wallet.clone(), 2000, None).await?;

    // Create two invoices
    let invoice1 = create_fake_invoice(200_000, "melt 1".to_string());
    let invoice2 = create_fake_invoice(300_000, "melt 2".to_string());

    // Get quotes
    let quote1 = wallet.melt_quote(invoice1.to_string(), None).await?;
    let quote2 = wallet.melt_quote(invoice2.to_string(), None).await?;

    // Execute both melts concurrently
    let wallet1 = wallet.clone();
    let wallet2 = wallet.clone();
    let quote_id1 = quote1.id.clone();
    let quote_id2 = quote2.id.clone();

    // Prepare both melts
    let prepared1 = wallet1
        .prepare_melt(&quote_id1, std::collections::HashMap::new())
        .await?;
    let prepared2 = wallet2
        .prepare_melt(&quote_id2, std::collections::HashMap::new())
        .await?;

    // Confirm both in parallel
    let (result1, result2) = tokio::join!(prepared1.confirm(), prepared2.confirm());

    // Both should succeed
    let confirmed1 = result1?;
    let confirmed2 = result2?;

    assert_eq!(confirmed1.state(), MeltQuoteState::Paid);
    assert_eq!(confirmed2.state(), MeltQuoteState::Paid);

    // Verify total amount melted
    let final_balance = wallet.total_balance().await?;
    assert!(final_balance < Amount::from(1500)); // At least 500 melted

    Ok(())
}

// =============================================================================
// Melt Saga Input Fee Tests
// =============================================================================

/// Tests that melt saga correctly includes input fees when calculating total needed.
///
/// This is a regression test for a bug where confirm_melt calculated:
///   inputs_needed_amount = quote.amount + fee_reserve
/// but should calculate:
///   inputs_needed_amount = quote.amount + fee_reserve + input_fee
///
/// The bug manifested as: "not enough inputs provided for melt. Provided: X, needed: X+1"
///
/// Scenario:
/// - Mint with 1000 ppk (1 sat per proof input fee)
/// - Melt for 26 sats
/// - fee_reserve = 2 sats
/// - If wallet has proofs that don't exactly match, it swaps first
/// - The swap produces proofs totaling (amount + fee_reserve) = 28 sats
/// - But mint actually needs (amount + fee_reserve + input_fee) = 29 sats
///
/// Before fix: Melt fails with "not enough inputs provided for melt"
/// After fix: Melt succeeds
#[tokio::test]
async fn test_melt_saga_includes_input_fees() -> Result<()> {
    use cdk::nuts::CurrencyUnit;

    setup_tracing();
    let mint = create_and_start_test_mint().await?;
    let wallet = create_test_wallet_for_mint(mint.clone()).await?;

    // Rotate to keyset with 1000 ppk = 1 sat per proof fee
    // This is required to trigger the bug - without input fees, the calculation is correct
    mint.rotate_keyset(
        CurrencyUnit::Sat,
        cdk_integration_tests::standard_keyset_amounts(32),
        1000, // 1 sat per proof input fee
    )
    .await
    .expect("Failed to rotate keyset");

    // Brief pause to ensure keyset rotation is complete
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Fund wallet with enough to cover melt amount + fee_reserve + input fees
    // Use larger amounts to ensure there are enough proofs of the right denominations
    let initial_amount = 500u64;
    fund_wallet(wallet.clone(), initial_amount, None).await?;

    let initial_balance = wallet.total_balance().await?;
    assert_eq!(initial_balance, Amount::from(initial_amount));

    // Create melt quote for an amount that requires a swap
    // 100 sats = 100000 msats
    // fee_reserve should be ~2 sats (2% of 100)
    // inputs_needed without input_fee = 102 sats
    // With input_fee (depends on proof count), mint needs more
    let invoice = create_fake_invoice(100_000, "test melt with fees".to_string());
    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await?;

    tracing::info!(
        "Melt quote: amount={}, fee_reserve={}",
        melt_quote.amount,
        melt_quote.fee_reserve
    );

    // Perform the melt - this should succeed even with input fees
    // Before the fix, this would fail with:
    // "not enough inputs provided for melt. Provided: X, needed: X+1"
    let prepared = wallet
        .prepare_melt(&melt_quote.id, std::collections::HashMap::new())
        .await?;
    let confirmed = prepared.confirm().await?;

    assert_eq!(confirmed.state(), MeltQuoteState::Paid);
    tracing::info!(
        "Melt succeeded: amount={}, fee_paid={}",
        confirmed.amount(),
        confirmed.fee_paid()
    );

    // Verify final balance makes sense
    let final_balance = wallet.total_balance().await?;
    assert!(
        final_balance < initial_balance,
        "Balance should decrease after melt"
    );

    Ok(())
}

/// Regression test: Melt with swap should account for actual output proof count.
///
/// This test reproduces a bug where:
/// 1. Wallet has many small proofs (non-optimal denominations)
/// 2. User tries to melt an amount that requires a swap
/// 3. The swap produces more proofs than the "optimal" estimate
/// 4. The actual input_fee is higher than estimated
/// 5. Result: "Insufficient funds" even though wallet has enough balance
///
/// The issue was that `estimated_melt_fee` was based on `inputs_needed_amount.split()`
/// but after swap with `amount=None`, the actual proof count could be higher,
/// leading to a higher `actual_input_fee`.
///
/// Example from real failure:
/// - inputs_needed_amount = 6700 (optimal split = 7 proofs, fee = 1)
/// - selection_amount = 6701
/// - Selected 12 proofs totaling 6703, swap_fee = 2
/// - After swap: 6701 worth but 13 proofs (not optimal 7!)
/// - actual_input_fee = 2 (not 1!)
/// - Need: 6633 + 67 + 2 = 6702, Have: 6701 → Insufficient funds!
#[tokio::test]
async fn test_melt_with_swap_non_optimal_proofs() -> Result<()> {
    use cdk::amount::SplitTarget;
    use cdk::nuts::CurrencyUnit;

    setup_tracing();
    let mint = create_and_start_test_mint().await?;
    let wallet = create_test_wallet_for_mint(mint.clone()).await?;

    // Use a keyset with 100 ppk (0.1 sat per proof, so ~10 proofs = 1 sat fee)
    // This makes the fee difference noticeable when proof count differs
    mint.rotate_keyset(
        CurrencyUnit::Sat,
        cdk_integration_tests::standard_keyset_amounts(32),
        100, // 0.1 sat per proof input fee
    )
    .await
    .expect("Failed to rotate keyset");

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Fund wallet with many 1-sat proofs (very non-optimal)
    // This forces a swap when trying to melt, and the swap output
    // may have more proofs than the "optimal" estimate
    let initial_amount = 200u64;
    fund_wallet(
        wallet.clone(),
        initial_amount,
        Some(SplitTarget::Value(Amount::ONE)),
    )
    .await?;

    let initial_balance = wallet.total_balance().await?;
    assert_eq!(initial_balance, Amount::from(initial_amount));

    // Verify we have many small proofs
    let proofs = wallet.get_unspent_proofs().await?;
    tracing::info!("Funded with {} proofs", proofs.len());
    assert!(
        proofs.len() > 50,
        "Should have many small proofs to force non-optimal swap"
    );

    // Create melt quote - amount chosen to require a swap
    // With 200 sats in 1-sat proofs, melting 100 sats should require swapping
    let invoice = create_fake_invoice(100_000, "test melt with non-optimal proofs".to_string());
    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await?;

    tracing::info!(
        "Melt quote: amount={}, fee_reserve={}",
        melt_quote.amount,
        melt_quote.fee_reserve
    );

    // This melt should succeed even with non-optimal proofs
    // Before fix: fails with "Insufficient funds" because actual_input_fee > estimated
    let prepared = wallet
        .prepare_melt(&melt_quote.id, std::collections::HashMap::new())
        .await?;
    let confirmed = prepared.confirm().await?;

    assert_eq!(confirmed.state(), MeltQuoteState::Paid);
    tracing::info!(
        "Melt succeeded: amount={}, fee_paid={}",
        confirmed.amount(),
        confirmed.fee_paid()
    );

    // Verify balance decreased appropriately
    let final_balance = wallet.total_balance().await?;
    assert!(
        final_balance < initial_balance,
        "Balance should decrease after melt"
    );

    Ok(())
}

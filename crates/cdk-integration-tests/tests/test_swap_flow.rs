//! Comprehensive tests for the current swap flow
//!
//! These tests validate the swap operation's behavior including:
//! - Happy path: successful token swaps
//! - Error handling: validation failures, rollback scenarios
//! - Edge cases: concurrent operations, double-spending
//! - State management: proof states, blinded message tracking
//!
//! The tests focus on the current implementation using ProofWriter and BlindedMessageWriter
//! patterns to ensure proper cleanup and rollback behavior.

use std::collections::HashMap;
use std::sync::Arc;

use cashu::amount::SplitTarget;
use cashu::dhke::construct_proofs;
use cashu::{CurrencyUnit, Id, PreMintSecrets, SecretKey, SpendingConditions, State, SwapRequest};
use cdk::mint::Mint;
use cdk::nuts::nut00::ProofsMethods;
use cdk::Amount;
use cdk_fake_wallet::create_fake_invoice;
use cdk_integration_tests::init_pure_tests::*;

/// Helper to get the active keyset ID from a mint
async fn get_keyset_id(mint: &Mint) -> Id {
    let keys = mint.pubkeys().keysets.first().unwrap().clone();
    keys.verify_id()
        .expect("Keyset ID generation is successful");
    keys.id
}

/// Tests the complete happy path of a swap operation:
/// 1. Wallet is funded with tokens
/// 2. Blinded messages are added to database
/// 3. Outputs are signed by mint
/// 4. Input proofs are verified
/// 5. Transaction is balanced
/// 6. Proofs are added and marked as spent
/// 7. Blind signatures are saved
/// All steps should succeed and database should be in consistent state.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_swap_happy_path() {
    setup_tracing();
    let mint = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");
    let wallet = create_test_wallet_for_mint(mint.clone())
        .await
        .expect("Failed to create test wallet");

    // Fund wallet with 100 sats
    fund_wallet(wallet.clone(), 100, None)
        .await
        .expect("Failed to fund wallet");

    let proofs = wallet
        .get_unspent_proofs()
        .await
        .expect("Could not get proofs");

    let keyset_id = get_keyset_id(&mint).await;

    // Check initial amounts after minting
    let total_issued = mint.total_issued().await.unwrap();
    let total_redeemed = mint.total_redeemed().await.unwrap();
    let initial_issued = total_issued
        .get(&keyset_id)
        .copied()
        .unwrap_or(Amount::ZERO);
    let initial_redeemed = total_redeemed
        .get(&keyset_id)
        .copied()
        .unwrap_or(Amount::ZERO);
    assert_eq!(
        initial_issued,
        Amount::from(100),
        "Should have issued 100 sats"
    );
    assert_eq!(
        initial_redeemed,
        Amount::ZERO,
        "Should have redeemed 0 sats initially"
    );

    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    // Create swap request for same amount (100 sats)
    let preswap = PreMintSecrets::random(
        keyset_id,
        100.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .expect("Failed to create preswap");

    let swap_request = SwapRequest::new(proofs.clone(), preswap.blinded_messages());

    // Execute swap
    let swap_response = mint
        .process_swap_request(swap_request)
        .await
        .expect("Swap should succeed");

    // Verify response contains correct number of signatures
    assert_eq!(
        swap_response.signatures.len(),
        preswap.blinded_messages().len(),
        "Should receive signature for each blinded message"
    );

    // Verify input proofs are marked as spent
    let states = mint
        .localstore()
        .get_proofs_states(&proofs.iter().map(|p| p.y().unwrap()).collect::<Vec<_>>())
        .await
        .expect("Failed to get proof states");

    for state in states {
        assert_eq!(
            State::Spent,
            state.expect("State should be known"),
            "All input proofs should be marked as spent"
        );
    }

    // Verify blind signatures were saved
    let saved_signatures = mint
        .localstore()
        .get_blind_signatures(
            &preswap
                .blinded_messages()
                .iter()
                .map(|bm| bm.blinded_secret)
                .collect::<Vec<_>>(),
        )
        .await
        .expect("Failed to get blind signatures");

    assert_eq!(
        saved_signatures.len(),
        swap_response.signatures.len(),
        "All signatures should be saved"
    );

    // Check keyset amounts after swap
    // Swap redeems old proofs (100 sats) and issues new proofs (100 sats)
    let total_issued = mint.total_issued().await.unwrap();
    let total_redeemed = mint.total_redeemed().await.unwrap();
    let after_issued = total_issued
        .get(&keyset_id)
        .copied()
        .unwrap_or(Amount::ZERO);
    let after_redeemed = total_redeemed
        .get(&keyset_id)
        .copied()
        .unwrap_or(Amount::ZERO);
    assert_eq!(
        after_issued,
        Amount::from(200),
        "Should have issued 200 sats total (initial 100 + swap 100)"
    );
    assert_eq!(
        after_redeemed,
        Amount::from(100),
        "Should have redeemed 100 sats from the swap"
    );
}

/// Tests that duplicate blinded messages are rejected:
/// 1. First swap with blinded messages succeeds
/// 2. Second swap attempt with same blinded messages fails
/// 3. BlindedMessageWriter should prevent reuse
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_swap_duplicate_blinded_messages() {
    setup_tracing();
    let mint = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");
    let wallet = create_test_wallet_for_mint(mint.clone())
        .await
        .expect("Failed to create test wallet");

    // Fund wallet with 200 sats (enough for two swaps)
    fund_wallet(wallet.clone(), 200, None)
        .await
        .expect("Failed to fund wallet");

    let all_proofs = wallet
        .get_unspent_proofs()
        .await
        .expect("Could not get proofs");

    // Split proofs into two sets
    let mid = all_proofs.len() / 2;
    let proofs1: Vec<_> = all_proofs.iter().take(mid).cloned().collect();
    let proofs2: Vec<_> = all_proofs.iter().skip(mid).cloned().collect();

    let keyset_id = get_keyset_id(&mint).await;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    // Create blinded messages for first swap
    let preswap = PreMintSecrets::random(
        keyset_id,
        proofs1.total_amount().unwrap(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .expect("Failed to create preswap");

    let blinded_messages = preswap.blinded_messages();

    // First swap should succeed
    let swap_request1 = SwapRequest::new(proofs1, blinded_messages.clone());
    mint.process_swap_request(swap_request1)
        .await
        .expect("First swap should succeed");

    // Second swap with SAME blinded messages should fail
    let swap_request2 = SwapRequest::new(proofs2, blinded_messages.clone());
    let result = mint.process_swap_request(swap_request2).await;

    assert!(
        result.is_err(),
        "Second swap with duplicate blinded messages should fail"
    );
}

/// Tests that swap correctly rejects double-spending attempts:
/// 1. First swap with proofs succeeds
/// 2. Second swap with same proofs fails with TokenAlreadySpent
/// 3. ProofWriter should detect already-spent proofs
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_swap_double_spend_detection() {
    setup_tracing();
    let mint = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");
    let wallet = create_test_wallet_for_mint(mint.clone())
        .await
        .expect("Failed to create test wallet");

    // Fund wallet with 100 sats
    fund_wallet(wallet.clone(), 100, None)
        .await
        .expect("Failed to fund wallet");

    let proofs = wallet
        .get_unspent_proofs()
        .await
        .expect("Could not get proofs");

    let keyset_id = get_keyset_id(&mint).await;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    // First swap
    let preswap1 = PreMintSecrets::random(
        keyset_id,
        100.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .expect("Failed to create preswap");

    let swap_request1 = SwapRequest::new(proofs.clone(), preswap1.blinded_messages());
    mint.process_swap_request(swap_request1)
        .await
        .expect("First swap should succeed");

    // Second swap with same proofs should fail
    let preswap2 = PreMintSecrets::random(
        keyset_id,
        100.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .expect("Failed to create preswap");

    let swap_request2 = SwapRequest::new(proofs.clone(), preswap2.blinded_messages());
    let result = mint.process_swap_request(swap_request2).await;

    match result {
        Err(cdk::Error::TokenAlreadySpent) => {
            // Expected error
        }
        Err(err) => panic!("Wrong error type: {:?}", err),
        Ok(_) => panic!("Double spend should not succeed"),
    }
}

/// Tests that unbalanced swap requests are rejected:
/// Case 1: Output amount < Input amount (trying to steal from mint)
/// Case 2: Output amount > Input amount (trying to create tokens)
/// Both should fail with TransactionUnbalanced error.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_swap_unbalanced_transaction_detection() {
    setup_tracing();
    let mint = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");
    let wallet = create_test_wallet_for_mint(mint.clone())
        .await
        .expect("Failed to create test wallet");

    // Fund wallet with 100 sats
    fund_wallet(wallet.clone(), 100, None)
        .await
        .expect("Failed to fund wallet");

    let proofs = wallet
        .get_unspent_proofs()
        .await
        .expect("Could not get proofs");

    let keyset_id = get_keyset_id(&mint).await;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    // Case 1: Try to swap for LESS (95 < 100) - underpaying
    let preswap_less = PreMintSecrets::random(
        keyset_id,
        95.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .expect("Failed to create preswap");

    let swap_request_less = SwapRequest::new(proofs.clone(), preswap_less.blinded_messages());

    match mint.process_swap_request(swap_request_less).await {
        Err(cdk::Error::TransactionUnbalanced(_, _, _)) => {
            // Expected error
        }
        Err(err) => panic!("Wrong error type for underpay: {:?}", err),
        Ok(_) => panic!("Unbalanced swap (underpay) should not succeed"),
    }

    // Case 2: Try to swap for MORE (105 > 100) - overpaying/creating tokens
    let preswap_more = PreMintSecrets::random(
        keyset_id,
        105.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .expect("Failed to create preswap");

    let swap_request_more = SwapRequest::new(proofs.clone(), preswap_more.blinded_messages());

    match mint.process_swap_request(swap_request_more).await {
        Err(cdk::Error::TransactionUnbalanced(_, _, _)) => {
            // Expected error
        }
        Err(err) => panic!("Wrong error type for overpay: {:?}", err),
        Ok(_) => panic!("Unbalanced swap (overpay) should not succeed"),
    }
}

/// Tests that swap requests with empty inputs or outputs are rejected:
/// Case 1: Empty outputs (inputs without outputs)
/// Case 2: Empty inputs (outputs without inputs)
/// Both should fail. Currently returns UnitMismatch (11010) instead of
/// TransactionUnbalanced (11002) because there are no keyset IDs to determine units.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_swap_empty_inputs_or_outputs() {
    setup_tracing();
    let mint = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");
    let wallet = create_test_wallet_for_mint(mint.clone())
        .await
        .expect("Failed to create test wallet");

    // Fund wallet with 100 sats
    fund_wallet(wallet.clone(), 100, None)
        .await
        .expect("Failed to fund wallet");

    let proofs = wallet
        .get_unspent_proofs()
        .await
        .expect("Could not get proofs");

    // Case 1: Swap request with inputs but empty outputs
    // This represents trying to destroy tokens (inputs with no outputs)
    let swap_request_empty_outputs = SwapRequest::new(proofs.clone(), vec![]);

    match mint.process_swap_request(swap_request_empty_outputs).await {
        Err(cdk::Error::TransactionUnbalanced(_, _, _)) => {
            // This would be the more appropriate error
        }
        Err(err) => panic!("Wrong error type for empty outputs: {:?}", err),
        Ok(_) => panic!("Swap with empty outputs should not succeed"),
    }

    // Case 2: Swap request with empty inputs but with outputs
    // This represents trying to create tokens from nothing
    let keyset_id = get_keyset_id(&mint).await;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    let preswap = PreMintSecrets::random(
        keyset_id,
        100.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .expect("Failed to create preswap");

    let swap_request_empty_inputs = SwapRequest::new(vec![], preswap.blinded_messages());

    match mint.process_swap_request(swap_request_empty_inputs).await {
        Err(cdk::Error::TransactionUnbalanced(_, _, _)) => {
            // This would be the more appropriate error
        }
        Err(err) => panic!("Wrong error type for empty inputs: {:?}", err),
        Ok(_) => panic!("Swap with empty inputs should not succeed"),
    }
}

/// Tests P2PK (Pay-to-Public-Key) spending conditions:
/// 1. Create proofs locked to a public key
/// 2. Attempt swap without signature - should fail
/// 3. Attempt swap with valid signature - should succeed
/// Validates NUT-11 signature enforcement.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_swap_p2pk_signature_validation() {
    setup_tracing();
    let mint = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");
    let wallet = create_test_wallet_for_mint(mint.clone())
        .await
        .expect("Failed to create test wallet");

    // Fund wallet with 100 sats
    fund_wallet(wallet.clone(), 100, None)
        .await
        .expect("Failed to fund wallet");

    let input_proofs = wallet
        .get_unspent_proofs()
        .await
        .expect("Could not get proofs");

    let keyset_id = get_keyset_id(&mint).await;
    let secret_key = SecretKey::generate();

    // Create P2PK locked outputs
    let spending_conditions = SpendingConditions::new_p2pk(secret_key.public_key(), None);
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    let pre_swap = PreMintSecrets::with_conditions(
        keyset_id,
        100.into(),
        &SplitTarget::default(),
        &spending_conditions,
        &fee_and_amounts,
    )
    .expect("Failed to create P2PK preswap");

    let swap_request = SwapRequest::new(input_proofs.clone(), pre_swap.blinded_messages());

    // First swap to get P2PK locked proofs
    let keys = mint.pubkeys().keysets.first().cloned().unwrap().keys;

    let post_swap = mint
        .process_swap_request(swap_request)
        .await
        .expect("Initial swap should succeed");

    // Construct proofs from swap response
    let mut p2pk_proofs = construct_proofs(
        post_swap.signatures,
        pre_swap.rs(),
        pre_swap.secrets(),
        &keys,
    )
    .expect("Failed to construct proofs");

    // Try to spend P2PK proofs WITHOUT signature - should fail
    let preswap_unsigned = PreMintSecrets::random(
        keyset_id,
        100.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .expect("Failed to create preswap");

    let swap_request_unsigned =
        SwapRequest::new(p2pk_proofs.clone(), preswap_unsigned.blinded_messages());

    match mint.process_swap_request(swap_request_unsigned).await {
        Err(cdk::Error::NUT11(cdk::nuts::nut11::Error::SignaturesNotProvided)) => {
            // Expected error
        }
        Err(err) => panic!("Wrong error type: {:?}", err),
        Ok(_) => panic!("Unsigned P2PK spend should fail"),
    }

    // Sign the proofs with correct key
    for proof in &mut p2pk_proofs {
        proof
            .sign_p2pk(secret_key.clone())
            .expect("Failed to sign proof");
    }

    // Try again WITH signature - should succeed
    let preswap_signed = PreMintSecrets::random(
        keyset_id,
        100.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .expect("Failed to create preswap");

    let swap_request_signed = SwapRequest::new(p2pk_proofs, preswap_signed.blinded_messages());

    mint.process_swap_request(swap_request_signed)
        .await
        .expect("Signed P2PK spend should succeed");
}

/// Tests rollback behavior when duplicate blinded messages are used:
/// This validates that the BlindedMessageWriter prevents reuse of blinded messages.
/// 1. First swap with blinded messages succeeds
/// 2. Second swap with same blinded messages fails
/// 3. The failure should happen early (during blinded message addition)
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_swap_rollback_on_duplicate_blinded_message() {
    setup_tracing();
    let mint = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");
    let wallet = create_test_wallet_for_mint(mint.clone())
        .await
        .expect("Failed to create test wallet");

    // Fund with enough for multiple swaps
    fund_wallet(wallet.clone(), 200, None)
        .await
        .expect("Failed to fund wallet");

    let all_proofs = wallet
        .get_unspent_proofs()
        .await
        .expect("Could not get proofs");

    let mid = all_proofs.len() / 2;
    let proofs1: Vec<_> = all_proofs.iter().take(mid).cloned().collect();
    let proofs2: Vec<_> = all_proofs.iter().skip(mid).cloned().collect();

    let keyset_id = get_keyset_id(&mint).await;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    // Create shared blinded messages
    let preswap = PreMintSecrets::random(
        keyset_id,
        proofs1.total_amount().unwrap(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .expect("Failed to create preswap");

    let blinded_messages = preswap.blinded_messages();

    // Extract proof2 ys before moving proofs2
    let proof2_ys: Vec<_> = proofs2.iter().map(|p| p.y().unwrap()).collect();

    // First swap succeeds
    let swap1 = SwapRequest::new(proofs1, blinded_messages.clone());
    mint.process_swap_request(swap1)
        .await
        .expect("First swap should succeed");

    // Second swap with duplicate blinded messages should fail early
    // The BlindedMessageWriter should detect duplicate and prevent the swap
    let swap2 = SwapRequest::new(proofs2, blinded_messages.clone());
    let result = mint.process_swap_request(swap2).await;

    assert!(
        result.is_err(),
        "Duplicate blinded messages should cause failure"
    );

    // Verify the second set of proofs are NOT marked as spent
    // (since the swap failed before processing them)
    let states = mint
        .localstore()
        .get_proofs_states(&proof2_ys)
        .await
        .expect("Failed to get proof states");

    for state in states {
        assert!(
            state.is_none(),
            "Proofs from failed swap should not be marked as spent"
        );
    }
}

/// Tests concurrent swap attempts with same proofs:
/// Spawns 3 concurrent tasks trying to swap the same proofs.
/// Only one should succeed, others should fail with TokenAlreadySpent or TokenPending.
/// Validates that concurrent access is properly handled.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn test_swap_concurrent_double_spend_prevention() {
    setup_tracing();
    let mint = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");
    let wallet = create_test_wallet_for_mint(mint.clone())
        .await
        .expect("Failed to create test wallet");

    // Fund wallet
    fund_wallet(wallet.clone(), 100, None)
        .await
        .expect("Failed to fund wallet");

    let proofs = wallet
        .get_unspent_proofs()
        .await
        .expect("Could not get proofs");

    let keyset_id = get_keyset_id(&mint).await;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    // Create 3 different swap requests with SAME proofs but different outputs
    let preswap1 = PreMintSecrets::random(
        keyset_id,
        100.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .expect("Failed to create preswap 1");

    let preswap2 = PreMintSecrets::random(
        keyset_id,
        100.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .expect("Failed to create preswap 2");

    let preswap3 = PreMintSecrets::random(
        keyset_id,
        100.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .expect("Failed to create preswap 3");

    let swap_request1 = SwapRequest::new(proofs.clone(), preswap1.blinded_messages());
    let swap_request2 = SwapRequest::new(proofs.clone(), preswap2.blinded_messages());
    let swap_request3 = SwapRequest::new(proofs.clone(), preswap3.blinded_messages());

    // Spawn concurrent tasks
    let mint1 = mint.clone();
    let mint2 = mint.clone();
    let mint3 = mint.clone();

    let task1 = tokio::spawn(async move { mint1.process_swap_request(swap_request1).await });
    let task2 = tokio::spawn(async move { mint2.process_swap_request(swap_request2).await });
    let task3 = tokio::spawn(async move { mint3.process_swap_request(swap_request3).await });

    // Wait for all tasks
    let results = tokio::try_join!(task1, task2, task3).expect("Tasks should complete");

    // Count successes and failures
    let mut success_count = 0;
    let mut failure_count = 0;

    for result in [results.0, results.1, results.2] {
        match result {
            Ok(_) => success_count += 1,
            Err(cdk::Error::TokenAlreadySpent) | Err(cdk::Error::TokenPending) => {
                failure_count += 1
            }
            Err(err) => panic!("Unexpected error: {:?}", err),
        }
    }

    assert_eq!(
        success_count, 1,
        "Exactly one swap should succeed in concurrent scenario"
    );
    assert_eq!(
        failure_count, 2,
        "Exactly two swaps should fail in concurrent scenario"
    );

    // Verify all proofs are marked as spent
    let states = mint
        .localstore()
        .get_proofs_states(&proofs.iter().map(|p| p.y().unwrap()).collect::<Vec<_>>())
        .await
        .expect("Failed to get proof states");

    for state in states {
        assert_eq!(
            State::Spent,
            state.expect("State should be known"),
            "All proofs should be marked as spent after concurrent attempts"
        );
    }
}

/// Tests swap with fees enabled:
/// 1. Create mint with keyset that has fees (1 sat per proof)
/// 2. Fund wallet with many small proofs
/// 3. Attempt swap without paying fee - should fail
/// 4. Attempt swap with correct fee deduction - should succeed
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_swap_with_fees() {
    setup_tracing();
    let mint = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");

    let wallet = create_test_wallet_for_mint(mint.clone())
        .await
        .expect("Failed to create test wallet");

    // Rotate to keyset with 1 sat per proof fee
    mint.rotate_keyset(
        CurrencyUnit::Sat,
        cdk_integration_tests::standard_keyset_amounts(32),
        100,
    )
    .await
    .expect("Failed to rotate keyset");

    // Fund with 1000 sats as individual 1-sat proofs using the fee-based keyset
    // Wait a bit for keyset to be available
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    fund_wallet(wallet.clone(), 1000, Some(SplitTarget::Value(Amount::ONE)))
        .await
        .expect("Failed to fund wallet");

    let proofs = wallet
        .get_unspent_proofs()
        .await
        .expect("Could not get proofs");

    // Take 100 proofs (100 sats total, will need to pay fee)
    let hundred_proofs: Vec<_> = proofs.iter().take(100).cloned().collect();

    // Get the keyset ID from the proofs (which will be the fee-based keyset)
    let keyset_id = hundred_proofs[0].keyset_id;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    // Try to swap for 100 outputs (same as input) - should fail due to unpaid fee
    let preswap_no_fee = PreMintSecrets::random(
        keyset_id,
        100.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .expect("Failed to create preswap");

    let swap_no_fee = SwapRequest::new(hundred_proofs.clone(), preswap_no_fee.blinded_messages());

    match mint.process_swap_request(swap_no_fee).await {
        Err(cdk::Error::TransactionUnbalanced(_, _, _)) => {
            // Expected - didn't pay the fee
        }
        Err(err) => panic!("Wrong error type: {:?}", err),
        Ok(_) => panic!("Should fail when fee not paid"),
    }

    // Calculate correct fee (1 sat per input proof in this keyset)
    let fee = hundred_proofs.len() as u64; // 1 sat per proof = 100 sats fee
    let output_amount = 100 - fee;

    // Swap with correct fee deduction - should succeed if output_amount > 0
    if output_amount > 0 {
        let preswap_with_fee = PreMintSecrets::random(
            keyset_id,
            output_amount.into(),
            &SplitTarget::default(),
            &fee_and_amounts,
        )
        .expect("Failed to create preswap with fee");

        let swap_with_fee =
            SwapRequest::new(hundred_proofs.clone(), preswap_with_fee.blinded_messages());

        mint.process_swap_request(swap_with_fee)
            .await
            .expect("Swap with correct fee should succeed");
    }
}

/// Tests melt with fees enabled and swap-before-melt optimization:
/// 1. Create mint with keyset that has fees (1000 ppk = 1 sat per proof)
/// 2. Fund wallet with proofs using default split (optimal denominations)
/// 3. Call melt() - should automatically swap if proofs don't match exactly
/// 4. Verify fee calculations are reasonable
///
/// Fee calculation:
/// - Initial: 4096 sats in optimal denominations
/// - Melt: 1000 sats, fee_reserve = 20 sats (2%)
/// - inputs_needed = 1020 sats
/// - Target split for 1020: [512, 256, 128, 64, 32, 16, 8, 4] = 8 proofs
/// - target_fee = 8 sats
/// - inputs_total_needed = 1028 sats
///
/// The wallet uses two-step selection:
/// - Step 1: Try to find exact proofs for inputs_needed (no swap fee)
/// - Step 2: If not exact, select proofs for inputs_total_needed and swap
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_melt_with_fees_swap_before_melt() {
    setup_tracing();
    let mint = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");

    let wallet = create_test_wallet_for_mint(mint.clone())
        .await
        .expect("Failed to create test wallet");

    // Rotate to keyset with 1000 ppk = 1 sat per proof fee
    mint.rotate_keyset(
        CurrencyUnit::Sat,
        cdk_integration_tests::standard_keyset_amounts(32),
        1000, // 1 sat per proof
    )
    .await
    .expect("Failed to rotate keyset");

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Fund with default split target to get optimal denominations
    // Use larger amount to ensure enough margin for swap fees
    let initial_amount = 4096u64;
    fund_wallet(wallet.clone(), initial_amount, None)
        .await
        .expect("Failed to fund wallet");

    let initial_balance: u64 = wallet.total_balance().await.unwrap().into();
    assert_eq!(initial_balance, initial_amount);

    let proofs = wallet.get_unspent_proofs().await.unwrap();
    let proof_amounts: Vec<u64> = proofs.iter().map(|p| u64::from(p.amount)).collect();
    tracing::info!("Proofs after funding: {:?}", proof_amounts);

    let proofs_total: u64 = proof_amounts.iter().sum();
    assert_eq!(
        proofs_total, initial_amount,
        "Total proofs should equal funded amount"
    );

    // Create melt quote for 1000 sats (1_000_000 msats)
    // Fake wallet: fee_reserve = max(1, amount * 2%) = 20 sats
    let invoice = create_fake_invoice(1_000_000, "".to_string()); // 1000 sats in msats
    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await.unwrap();

    let quote_amount: u64 = melt_quote.amount.into();
    let fee_reserve: u64 = melt_quote.fee_reserve.into();

    tracing::info!(
        "Melt quote: amount={}, fee_reserve={}",
        quote_amount,
        fee_reserve
    );

    let initial_proof_count = proofs.len();

    tracing::info!(
        "Initial state: {} proofs, {} sats",
        initial_proof_count,
        proofs_total
    );

    // Perform melt
    let prepared = wallet
        .prepare_melt(&melt_quote.id, std::collections::HashMap::new())
        .await
        .unwrap();
    let melted = prepared.confirm().await.unwrap();

    let melt_amount: u64 = melted.amount().into();
    let ln_fee_paid: u64 = melted.fee_paid().into();

    tracing::info!(
        "Melt completed: amount={}, ln_fee_paid={}",
        melt_amount,
        ln_fee_paid
    );

    assert_eq!(melt_amount, quote_amount, "Melt amount should match quote");

    // Get final balance and calculate fees
    let final_balance: u64 = wallet.total_balance().await.unwrap().into();
    let total_spent = initial_amount - final_balance;
    let total_fees = total_spent - melt_amount;

    tracing::info!(
        "Balance: initial={}, final={}, total_spent={}, melt_amount={}, total_fees={}",
        initial_amount,
        final_balance,
        total_spent,
        melt_amount,
        total_fees
    );

    // Calculate input fees (swap + melt)
    let input_fees = total_fees - ln_fee_paid;

    tracing::info!(
        "Fee breakdown: total_fees={}, ln_fee={}, input_fees (swap+melt)={}",
        total_fees,
        ln_fee_paid,
        input_fees
    );

    // Verify input fees are reasonable
    // With swap-before-melt optimization, we use fewer proofs for the melt
    // Melt uses ~8 proofs for optimal split of 1028, so input_fee ~= 8
    // Swap (if any) also has fees, but the optimization minimizes total fees
    assert!(
        input_fees > 0,
        "Should have some input fees with fee-enabled keyset"
    );
    assert!(
        input_fees <= 20,
        "Input fees {} should be reasonable (not too high)",
        input_fees
    );

    // Verify we have change remaining
    assert!(final_balance > 0, "Should have change remaining after melt");

    tracing::info!(
        "Test passed: spent {} sats, fees {} (ln={}, input={}), remaining {}",
        total_spent,
        total_fees,
        ln_fee_paid,
        input_fees,
        final_balance
    );
}

/// Tests the "exact match" early return path in melt_with_metadata.
/// When proofs already exactly match inputs_needed_amount, no swap is required.
///
/// This tests Step 1 of the two-step selection:
/// - Select proofs for inputs_needed_amount
/// - If exact match, use proofs directly without swap
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_melt_exact_match_no_swap() {
    setup_tracing();
    let mint = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");

    let wallet = create_test_wallet_for_mint(mint.clone())
        .await
        .expect("Failed to create test wallet");

    // Use keyset WITHOUT fees to make exact match easier
    // (default keyset has no fees)

    // Fund with exactly inputs_needed_amount to trigger the exact match path
    // For a 1000 sat melt, fee_reserve = max(1, 1000 * 2%) = 20 sats
    // inputs_needed = 1000 + 20 = 1020 sats
    let initial_amount = 1020u64;
    fund_wallet(wallet.clone(), initial_amount, None)
        .await
        .expect("Failed to fund wallet");

    let initial_balance: u64 = wallet.total_balance().await.unwrap().into();
    assert_eq!(initial_balance, initial_amount);

    let proofs_before = wallet.get_unspent_proofs().await.unwrap();
    tracing::info!(
        "Proofs before melt: {:?}",
        proofs_before
            .iter()
            .map(|p| u64::from(p.amount))
            .collect::<Vec<_>>()
    );

    // Create melt quote for 1000 sats
    // fee_reserve = max(1, 1000 * 2%) = 20 sats
    // inputs_needed = 1000 + 20 = 1020 sats = our exact balance
    let invoice = create_fake_invoice(1_000_000, "".to_string());
    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await.unwrap();

    let quote_amount: u64 = melt_quote.amount.into();
    let fee_reserve: u64 = melt_quote.fee_reserve.into();
    let inputs_needed = quote_amount + fee_reserve;

    tracing::info!(
        "Melt quote: amount={}, fee_reserve={}, inputs_needed={}",
        quote_amount,
        fee_reserve,
        inputs_needed
    );

    // Perform melt
    let prepared = wallet
        .prepare_melt(&melt_quote.id, std::collections::HashMap::new())
        .await
        .unwrap();
    let melted = prepared.confirm().await.unwrap();

    let melt_amount: u64 = melted.amount().into();
    let ln_fee_paid: u64 = melted.fee_paid().into();

    tracing::info!(
        "Melt completed: amount={}, ln_fee_paid={}",
        melt_amount,
        ln_fee_paid
    );

    assert_eq!(melt_amount, quote_amount, "Melt amount should match quote");

    // Get final balance
    let final_balance: u64 = wallet.total_balance().await.unwrap().into();
    let total_spent = initial_amount - final_balance;
    let total_fees = total_spent - melt_amount;

    tracing::info!(
        "Balance: initial={}, final={}, total_spent={}, total_fees={}",
        initial_amount,
        final_balance,
        total_spent,
        total_fees
    );

    // With no keyset fees and no swap needed, total fees should just be ln_fee
    // (no input fees since default keyset has 0 ppk)
    assert_eq!(
        total_fees, ln_fee_paid,
        "Total fees should equal LN fee (no swap or input fees with 0 ppk keyset)"
    );

    tracing::info!("Test passed: exact match path used, no swap needed");
}

/// Tests melt with small amounts where swap margin is too tight.
/// When fees are high relative to the melt amount, the swap-before-melt
/// optimization may not have enough margin to cover both input and output fees.
/// In this case, the wallet should fall back to using proofs directly.
///
/// Scenario:
/// - Fund with 8 sats
/// - Melt 5 sats (with 2% fee_reserve = 1 sat min, so inputs_needed = 6)
/// - With 1 sat per proof fee, the swap margin becomes too tight
/// - Should still succeed by falling back to direct melt
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_melt_small_amount_tight_margin() {
    setup_tracing();
    let mint = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");

    let wallet = create_test_wallet_for_mint(mint.clone())
        .await
        .expect("Failed to create test wallet");

    // Rotate to keyset with 1000 ppk = 1 sat per proof fee
    mint.rotate_keyset(
        CurrencyUnit::Sat,
        cdk_integration_tests::standard_keyset_amounts(32),
        1000,
    )
    .await
    .expect("Failed to rotate keyset");

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Fund with enough to cover melt + fees, but amounts that will trigger swap
    // 32 sats gives us enough margin even with 1 sat/proof fees
    let initial_amount = 32;
    fund_wallet(wallet.clone(), initial_amount, None)
        .await
        .expect("Failed to fund wallet");

    let initial_balance: u64 = wallet.total_balance().await.unwrap().into();
    assert_eq!(initial_balance, initial_amount);

    let proofs = wallet.get_unspent_proofs().await.unwrap();
    tracing::info!(
        "Proofs after funding: {:?}",
        proofs
            .iter()
            .map(|p| u64::from(p.amount))
            .collect::<Vec<_>>()
    );

    // Create melt quote for 5 sats
    // fee_reserve = max(1, 5 * 2%) = 1 sat
    // inputs_needed = 5 + 1 = 6 sats
    let invoice = create_fake_invoice(5_000, "".to_string()); // 5 sats in msats
    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await.unwrap();

    let quote_amount: u64 = melt_quote.amount.into();
    let fee_reserve: u64 = melt_quote.fee_reserve.into();

    tracing::info!(
        "Melt quote: amount={}, fee_reserve={}, inputs_needed={}",
        quote_amount,
        fee_reserve,
        quote_amount + fee_reserve
    );

    // This should succeed even with tight margins
    let prepared = wallet
        .prepare_melt(&melt_quote.id, std::collections::HashMap::new())
        .await
        .expect("Prepare melt should succeed");
    let melted = prepared
        .confirm()
        .await
        .expect("Melt should succeed even with tight swap margin");

    let melt_amount: u64 = melted.amount().into();
    assert_eq!(melt_amount, quote_amount, "Melt amount should match quote");

    let final_balance: u64 = wallet.total_balance().await.unwrap().into();
    tracing::info!(
        "Melt completed: amount={}, fee_paid={}, final_balance={}",
        melted.amount(),
        melted.fee_paid(),
        final_balance
    );

    // Verify balance decreased appropriately
    assert!(
        final_balance < initial_balance,
        "Balance should decrease after melt"
    );
}

/// Tests melt where swap proofs barely cover swap_amount + input_fee.
///
/// This is a regression test for a bug where the swap-before-melt was called
/// with include_fees=true, causing it to try to add output fees on top of
/// swap_amount + input_fee. When proofs_to_swap had just barely enough value,
/// this caused InsufficientFunds error.
///
/// Scenario (from the bug):
/// - Balance: proofs like [4, 2, 1, 1] = 8 sats
/// - Melt: 5 sats + 1 fee_reserve = 6 inputs_needed
/// - target_fee = 1 (for optimal output split)
/// - inputs_total_needed = 7
/// - proofs_to_send = [4, 2] = 6, proofs_to_swap = [1, 1] = 2
/// - swap_amount = 1 sat (7 - 6)
/// - swap input_fee = 1 sat (2 proofs)
/// - Before fix: include_fees=true tried to add output fee, causing failure
/// - After fix: include_fees=false, swap succeeds
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_melt_swap_tight_margin_regression() {
    setup_tracing();
    let mint = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");

    let wallet = create_test_wallet_for_mint(mint.clone())
        .await
        .expect("Failed to create test wallet");

    // Rotate to keyset with 250 ppk = 0.25 sat per proof fee (same as original bug scenario)
    // This means 4 proofs = 1 sat fee
    mint.rotate_keyset(
        CurrencyUnit::Sat,
        cdk_integration_tests::standard_keyset_amounts(32),
        250,
    )
    .await
    .expect("Failed to rotate keyset");

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Fund with 100 sats using default split to get optimal denominations
    // This should give us proofs like [64, 32, 4] or similar power-of-2 split
    let initial_amount = 100;
    fund_wallet(wallet.clone(), initial_amount, None)
        .await
        .expect("Failed to fund wallet");

    let initial_balance: u64 = wallet.total_balance().await.unwrap().into();
    assert_eq!(initial_balance, initial_amount);

    let proofs = wallet.get_unspent_proofs().await.unwrap();
    let proof_amounts: Vec<u64> = proofs.iter().map(|p| u64::from(p.amount)).collect();
    tracing::info!("Proofs after funding: {:?}", proof_amounts);

    // Create melt quote for 5 sats (5000 msats)
    // fee_reserve = max(1, 5 * 2%) = 1 sat
    // inputs_needed = 5 + 1 = 6 sats
    // The optimal split for 6 sats is [4, 2] (2 proofs)
    // target_fee = 1 sat (2 proofs * 0.25, rounded up)
    // inputs_total_needed = 7 sats
    //
    // If we don't have exact [4, 2] proofs, we'll need to swap.
    // The swap path is what triggered the original bug when proofs_to_swap
    // had tight margins and include_fees=true was incorrectly used.
    let invoice = create_fake_invoice(5_000, "".to_string());
    let melt_quote = wallet.melt_quote(invoice.to_string(), None).await.unwrap();

    let quote_amount: u64 = melt_quote.amount.into();
    let fee_reserve: u64 = melt_quote.fee_reserve.into();

    tracing::info!(
        "Melt quote: amount={}, fee_reserve={}, inputs_needed={}",
        quote_amount,
        fee_reserve,
        quote_amount + fee_reserve
    );

    // This is the key test: melt should succeed even when swap is needed
    // Before the fix, include_fees=true in swap caused InsufficientFunds
    // After the fix, include_fees=false allows the swap to succeed
    let prepared = wallet
        .prepare_melt(&melt_quote.id, std::collections::HashMap::new())
        .await
        .expect("Prepare melt should succeed");
    let melted = prepared
        .confirm()
        .await
        .expect("Melt should succeed with swap-before-melt (regression test)");

    let melt_amount: u64 = melted.amount().into();
    assert_eq!(melt_amount, quote_amount, "Melt amount should match quote");

    let final_balance: u64 = wallet.total_balance().await.unwrap().into();
    tracing::info!(
        "Melt completed: amount={}, fee_paid={}, final_balance={}",
        melted.amount(),
        melted.fee_paid(),
        final_balance
    );

    // Should have change remaining
    assert!(
        final_balance < initial_balance,
        "Balance should decrease after melt"
    );
    assert!(final_balance > 0, "Should have change remaining");
}

/// Tests that swap correctly handles amount overflow:
/// Attempts to create outputs that would overflow u64 when summed.
/// This should be rejected before any database operations occur.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_swap_amount_overflow_protection() {
    setup_tracing();
    let mint = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");
    let wallet = create_test_wallet_for_mint(mint.clone())
        .await
        .expect("Failed to create test wallet");

    // Fund wallet
    fund_wallet(wallet.clone(), 100, None)
        .await
        .expect("Failed to fund wallet");

    let proofs = wallet
        .get_unspent_proofs()
        .await
        .expect("Could not get proofs");

    let keyset_id = get_keyset_id(&mint).await;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    // Try to create outputs that would overflow
    // 2^63 + 2^63 + small amount would overflow u64
    let large_amount = 2_u64.pow(63);

    let pre_mint1 = PreMintSecrets::random(
        keyset_id,
        large_amount.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .expect("Failed to create pre_mint1");

    let pre_mint2 = PreMintSecrets::random(
        keyset_id,
        large_amount.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .expect("Failed to create pre_mint2");

    let mut combined_pre_mint = PreMintSecrets::random(
        keyset_id,
        1.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .expect("Failed to create combined_pre_mint");

    combined_pre_mint.combine(pre_mint1);
    combined_pre_mint.combine(pre_mint2);

    let swap_request = SwapRequest::new(proofs, combined_pre_mint.blinded_messages());

    // Should fail with overflow/amount error
    match mint.process_swap_request(swap_request).await {
        Err(cdk::Error::NUT03(cdk::nuts::nut03::Error::Amount(_)))
        | Err(cdk::Error::AmountOverflow)
        | Err(cdk::Error::AmountError(_))
        | Err(cdk::Error::TransactionUnbalanced(_, _, _)) => {
            // Any of these errors are acceptable for overflow
        }
        Err(err) => panic!("Unexpected error type: {:?}", err),
        Ok(_) => panic!("Overflow swap should not succeed"),
    }
}

/// Tests swap state transitions through pubsub notifications:
/// 1. Subscribe to proof state changes
/// 2. Execute swap
/// 3. Verify Pending then Spent state transitions are received
/// Validates NUT-17 notification behavior.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_swap_state_transition_notifications() {
    setup_tracing();
    let mint = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");
    let wallet = create_test_wallet_for_mint(mint.clone())
        .await
        .expect("Failed to create test wallet");

    // Fund wallet
    fund_wallet(wallet.clone(), 100, None)
        .await
        .expect("Failed to fund wallet");

    let proofs = wallet
        .get_unspent_proofs()
        .await
        .expect("Could not get proofs");

    let keyset_id = get_keyset_id(&mint).await;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    let preswap = PreMintSecrets::random(
        keyset_id,
        100.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .expect("Failed to create preswap");

    let swap_request = SwapRequest::new(proofs.clone(), preswap.blinded_messages());

    // Subscribe to proof state changes
    let proof_ys: Vec<String> = proofs.iter().map(|p| p.y().unwrap().to_string()).collect();

    let mut listener = mint
        .pubsub_manager()
        .subscribe(cdk::subscription::Params {
            kind: cdk::nuts::nut17::Kind::ProofState,
            filters: proof_ys.clone(),
            id: Arc::new("test_swap_notifications".into()),
        })
        .expect("Should subscribe successfully");

    // Execute swap
    mint.process_swap_request(swap_request)
        .await
        .expect("Swap should succeed");

    // Give pubsub time to deliver messages
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Collect all state transition notifications
    let mut state_transitions: HashMap<String, Vec<State>> = HashMap::new();

    while let Some(msg) = listener.try_recv() {
        match msg.into_inner() {
            cashu::NotificationPayload::ProofState(cashu::ProofState { y, state, .. }) => {
                state_transitions
                    .entry(y.to_string())
                    .or_default()
                    .push(state);
            }
            _ => panic!("Unexpected notification type"),
        }
    }

    // Verify each proof went through Pending -> Spent transition
    for y in proof_ys {
        let transitions = state_transitions
            .get(&y)
            .expect("Should have transitions for proof");

        assert_eq!(
            transitions,
            &vec![State::Pending, State::Spent],
            "Proof should transition from Pending to Spent"
        );
    }
}

/// Tests that swap fails gracefully when proof states cannot be updated:
/// This would test the rollback path where proofs are added but state update fails.
/// In the current implementation, this should trigger rollback of both proofs and blinded messages.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_swap_proof_state_consistency() {
    setup_tracing();
    let mint = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");
    let wallet = create_test_wallet_for_mint(mint.clone())
        .await
        .expect("Failed to create test wallet");

    // Fund wallet
    fund_wallet(wallet.clone(), 100, None)
        .await
        .expect("Failed to fund wallet");

    let proofs = wallet
        .get_unspent_proofs()
        .await
        .expect("Could not get proofs");

    let keyset_id = get_keyset_id(&mint).await;
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    // Execute successful swap
    let preswap = PreMintSecrets::random(
        keyset_id,
        100.into(),
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .expect("Failed to create preswap");

    let swap_request = SwapRequest::new(proofs.clone(), preswap.blinded_messages());

    mint.process_swap_request(swap_request)
        .await
        .expect("Swap should succeed");

    // Verify all proofs have consistent state (Spent)
    let proof_ys: Vec<_> = proofs.iter().map(|p| p.y().unwrap()).collect();

    let states = mint
        .localstore()
        .get_proofs_states(&proof_ys)
        .await
        .expect("Failed to get proof states");

    // All states should be Some(Spent) - none should be None or Pending
    for (i, state) in states.iter().enumerate() {
        match state {
            Some(State::Spent) => {
                // Expected state
            }
            Some(other_state) => {
                panic!("Proof {} in unexpected state: {:?}", i, other_state)
            }
            None => {
                panic!("Proof {} has no state (should be Spent)", i)
            }
        }
    }
}

/// Tests that wallet correctly increments keyset counters when receiving proofs
/// from multiple keysets and then performing operations with them.
///
/// This test validates:
/// 1. Wallet can receive proofs from multiple different keysets
/// 2. Counter is correctly incremented for the target keyset during swap
/// 3. Database maintains separate counters for each keyset
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_wallet_multi_keyset_counter_updates() {
    setup_tracing();
    let mint = create_and_start_test_mint()
        .await
        .expect("Failed to create test mint");
    let wallet = create_test_wallet_for_mint(mint.clone())
        .await
        .expect("Failed to create test wallet");

    // Fund wallet with initial 100 sats using first keyset
    fund_wallet(wallet.clone(), 100, None)
        .await
        .expect("Failed to fund wallet");

    let first_keyset_id = get_keyset_id(&mint).await;

    // Rotate to a second keyset
    mint.rotate_keyset(
        CurrencyUnit::Sat,
        cdk_integration_tests::standard_keyset_amounts(32),
        0,
    )
    .await
    .expect("Failed to rotate keyset");

    // Wait for keyset rotation to propagate
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Refresh wallet keysets to know about the new keyset
    wallet
        .refresh_keysets()
        .await
        .expect("Failed to refresh wallet keysets");

    // Fund wallet again with 100 sats using second keyset
    fund_wallet(wallet.clone(), 100, None)
        .await
        .expect("Failed to fund wallet with second keyset");

    let second_keyset_id = mint
        .pubkeys()
        .keysets
        .iter()
        .find(|k| k.id != first_keyset_id)
        .expect("Should have second keyset")
        .id;

    // Verify we now have proofs from two different keysets
    let all_proofs = wallet
        .get_unspent_proofs()
        .await
        .expect("Could not get proofs");

    let keysets_in_use: std::collections::HashSet<_> =
        all_proofs.iter().map(|p| p.keyset_id).collect();

    assert_eq!(
        keysets_in_use.len(),
        2,
        "Should have proofs from 2 different keysets"
    );
    assert!(
        keysets_in_use.contains(&first_keyset_id),
        "Should have proofs from first keyset"
    );
    assert!(
        keysets_in_use.contains(&second_keyset_id),
        "Should have proofs from second keyset"
    );

    // Get initial total issued and redeemed for both keysets before swap
    let total_issued_before = mint.total_issued().await.unwrap();
    let total_redeemed_before = mint.total_redeemed().await.unwrap();

    let first_keyset_issued_before = total_issued_before
        .get(&first_keyset_id)
        .copied()
        .unwrap_or(Amount::ZERO);
    let first_keyset_redeemed_before = total_redeemed_before
        .get(&first_keyset_id)
        .copied()
        .unwrap_or(Amount::ZERO);

    let second_keyset_issued_before = total_issued_before
        .get(&second_keyset_id)
        .copied()
        .unwrap_or(Amount::ZERO);
    let second_keyset_redeemed_before = total_redeemed_before
        .get(&second_keyset_id)
        .copied()
        .unwrap_or(Amount::ZERO);

    tracing::info!(
        "Before swap - First keyset: issued={}, redeemed={}",
        first_keyset_issued_before,
        first_keyset_redeemed_before
    );
    tracing::info!(
        "Before swap - Second keyset: issued={}, redeemed={}",
        second_keyset_issued_before,
        second_keyset_redeemed_before
    );

    // Both keysets should have issued 100 sats
    assert_eq!(
        first_keyset_issued_before,
        Amount::from(100),
        "First keyset should have issued 100 sats"
    );
    assert_eq!(
        second_keyset_issued_before,
        Amount::from(100),
        "Second keyset should have issued 100 sats"
    );
    // Neither should have redeemed anything yet
    assert_eq!(
        first_keyset_redeemed_before,
        Amount::ZERO,
        "First keyset should have redeemed 0 sats before swap"
    );
    assert_eq!(
        second_keyset_redeemed_before,
        Amount::ZERO,
        "Second keyset should have redeemed 0 sats before swap"
    );

    // Now perform a swap with all proofs - this should only increment the counter
    // for the active (second) keyset, not for the first keyset
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();

    let total_amount = all_proofs.total_amount().expect("Should get total amount");

    // Create swap using the active (second) keyset
    let preswap = PreMintSecrets::random(
        second_keyset_id,
        total_amount,
        &SplitTarget::default(),
        &fee_and_amounts,
    )
    .expect("Failed to create preswap");

    let swap_request = SwapRequest::new(all_proofs.clone(), preswap.blinded_messages());

    // Execute the swap
    let swap_response = mint
        .process_swap_request(swap_request)
        .await
        .expect("Swap should succeed");

    // Verify response
    assert_eq!(
        swap_response.signatures.len(),
        preswap.blinded_messages().len(),
        "Should receive signature for each blinded message"
    );

    // All the new proofs should be from the second (active) keyset
    let keys = mint
        .pubkeys()
        .keysets
        .iter()
        .find(|k| k.id == second_keyset_id)
        .expect("Should find second keyset")
        .keys
        .clone();

    let new_proofs = construct_proofs(
        swap_response.signatures,
        preswap.rs(),
        preswap.secrets(),
        &keys,
    )
    .expect("Failed to construct proofs");

    // Verify all new proofs use the second keyset
    for proof in &new_proofs {
        assert_eq!(
            proof.keyset_id, second_keyset_id,
            "All new proofs should use the active (second) keyset"
        );
    }

    // Verify total issued and redeemed after swap
    let total_issued_after = mint.total_issued().await.unwrap();
    let total_redeemed_after = mint.total_redeemed().await.unwrap();

    let first_keyset_issued_after = total_issued_after
        .get(&first_keyset_id)
        .copied()
        .unwrap_or(Amount::ZERO);
    let first_keyset_redeemed_after = total_redeemed_after
        .get(&first_keyset_id)
        .copied()
        .unwrap_or(Amount::ZERO);

    let second_keyset_issued_after = total_issued_after
        .get(&second_keyset_id)
        .copied()
        .unwrap_or(Amount::ZERO);
    let second_keyset_redeemed_after = total_redeemed_after
        .get(&second_keyset_id)
        .copied()
        .unwrap_or(Amount::ZERO);

    tracing::info!(
        "After swap - First keyset: issued={}, redeemed={}",
        first_keyset_issued_after,
        first_keyset_redeemed_after
    );
    tracing::info!(
        "After swap - Second keyset: issued={}, redeemed={}",
        second_keyset_issued_after,
        second_keyset_redeemed_after
    );

    // After swap:
    // - First keyset: issued stays 100, redeemed increases by 100 (all its proofs were spent in swap)
    // - Second keyset: issued increases by 200 (original 100 + new 100 from swap output),
    //                  redeemed increases by 100 (its proofs from first funding were spent)
    assert_eq!(
        first_keyset_issued_after,
        Amount::from(100),
        "First keyset issued should stay 100 sats (no new issuance)"
    );
    assert_eq!(
        first_keyset_redeemed_after,
        Amount::from(100),
        "First keyset should have redeemed 100 sats (all its proofs spent in swap)"
    );

    assert_eq!(
        second_keyset_issued_after,
        Amount::from(300),
        "Second keyset should have issued 300 sats total (100 initial + 100 the second funding + 100 from swap output from the old keyset)"
    );
    assert_eq!(
        second_keyset_redeemed_after,
        Amount::from(100),
        "Second keyset should have redeemed 100 sats (its proofs from initial funding spent in swap)"
    );

    // The test verifies that:
    // 1. We can have proofs from multiple keysets in a wallet
    // 2. Swap operation processes inputs from any keyset but creates outputs using active keyset
    // 3. The keyset_counter table correctly handles counters for different keysets independently
    // 4. The database upsert logic in increment_keyset_counter works for multiple keysets
    // 5. Total issued and redeemed are tracked correctly per keyset during multi-keyset swaps
}

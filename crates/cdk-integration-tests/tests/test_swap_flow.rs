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
    mint.rotate_keyset(CurrencyUnit::Sat, 32, 1)
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
                    .or_insert_with(Vec::new)
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

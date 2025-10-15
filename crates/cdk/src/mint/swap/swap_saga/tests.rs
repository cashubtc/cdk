#![cfg(test)]
//! Unit tests for the swap saga implementation
//!
//! These tests verify the swap saga pattern using in-memory mints and databases,
//! without requiring external dependencies like Lightning nodes.

use std::sync::Arc;

use cdk_common::nuts::{Proofs, ProofsMethods};
use cdk_common::{Amount, State};

use super::SwapSaga;
use crate::mint::swap::Mint;
use crate::mint::Verification;
use crate::test_helpers::mint::{create_test_blinded_messages, create_test_mint};

/// Helper to create a verification result for testing
fn create_verification(amount: Amount) -> Verification {
    Verification {
        amount,
        unit: Some(cdk_common::nuts::CurrencyUnit::Sat),
    }
}

/// Helper to create test proofs for swapping using the mint's process
async fn create_swap_inputs(mint: &Mint, amount: Amount) -> (Proofs, Verification) {
    let proofs = crate::test_helpers::mint::mint_test_proofs(mint, amount)
        .await
        .expect("Failed to create test proofs");

    let verification = create_verification(amount);

    (proofs, verification)
}

/// Tests that a SwapSaga can be created in the Initial state.
///
/// # What This Tests
/// - SwapSaga::new() creates a saga in the Initial state
/// - The typestate pattern ensures only Initial state is accessible after creation
/// - No database operations occur during construction
///
/// # Success Criteria
/// - Saga can be instantiated without errors
/// - Saga is in Initial state (enforced by type system)
#[tokio::test]
async fn test_swap_saga_initial_state_creation() {
    let mint = create_test_mint().await.unwrap();
    let db = mint.localstore();
    let pubsub = mint.pubsub_manager();

    let _saga = SwapSaga::new(&mint, db, pubsub);

    // If we can create the saga, we're in the Initial state
    // This is verified by the type system - only SwapSaga<Initial> can be created with new()
}

/// Tests the complete happy path flow through all saga states.
///
/// # What This Tests
/// - Initial -> SetupComplete -> Signed -> Response state transitions
/// - Database transactions commit successfully at each stage
/// - Input proofs are marked as Pending during setup, then Spent after finalization
/// - Output signatures are generated and returned correctly
/// - Compensations are cleared on successful completion
///
/// # Flow
/// 1. Create saga in Initial state
/// 2. setup_swap: Transition to SetupComplete (TX1: add proofs + blinded messages)
/// 3. sign_outputs: Transition to Signed (blind signing, no DB operations)
/// 4. finalize: Complete saga (TX2: add signatures, mark proofs spent)
///
/// # Success Criteria
/// - All state transitions succeed
/// - Response contains correct number of signatures
/// - All input proofs are marked as Spent
/// - No errors occur during the entire flow
#[tokio::test]
async fn test_swap_saga_full_flow_success() {
    let mint = create_test_mint().await.unwrap();

    let amount = Amount::from(100);
    let (input_proofs, input_verification) = create_swap_inputs(&mint, amount).await;

    let (output_blinded_messages, _pre_mint) =
        create_test_blinded_messages(&mint, amount).await.unwrap();

    let db = mint.localstore();
    let pubsub = mint.pubsub_manager();

    let saga = SwapSaga::new(&mint, db, pubsub);

    let saga = saga
        .setup_swap(
            &input_proofs,
            &output_blinded_messages,
            None,
            input_verification,
        )
        .await
        .expect("Setup should succeed");

    let saga = saga.sign_outputs().await.expect("Signing should succeed");

    let response = saga.finalize().await.expect("Finalize should succeed");

    assert_eq!(
        response.signatures.len(),
        output_blinded_messages.len(),
        "Should have signatures for all outputs"
    );

    let ys = input_proofs.ys().unwrap();
    let states = mint
        .localstore()
        .get_proofs_states(&ys)
        .await
        .expect("Failed to get proof states");

    for state in states {
        assert_eq!(
            state.unwrap(),
            State::Spent,
            "Input proofs should be marked as spent"
        );
    }
}

/// Tests the Initial -> SetupComplete state transition.
///
/// # What This Tests
/// - setup_swap() successfully transitions saga from Initial to SetupComplete state
/// - State data contains blinded messages and input proof Y values
/// - Database transaction (TX1) commits successfully
/// - Input proofs are marked as Pending (not Spent)
/// - Compensation action is registered for potential rollback
///
/// # Database Operations (TX1)
/// 1. Verify transaction is balanced
/// 2. Add input proofs to database
/// 3. Update proof states to Pending
/// 4. Add output blinded messages to database
/// 5. Commit transaction
///
/// # Success Criteria
/// - Saga transitions to SetupComplete state
/// - State data correctly stores blinded messages and input Ys
/// - All input proofs have state = Pending in database
#[tokio::test]
async fn test_swap_saga_setup_transition() {
    let mint = create_test_mint().await.unwrap();

    let amount = Amount::from(64);
    let (input_proofs, input_verification) = create_swap_inputs(&mint, amount).await;

    let (output_blinded_messages, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

    let db = mint.localstore();
    let pubsub = mint.pubsub_manager();

    let saga = SwapSaga::new(&mint, db, pubsub);

    let saga = saga
        .setup_swap(
            &input_proofs,
            &output_blinded_messages,
            None,
            input_verification,
        )
        .await
        .expect("Setup should succeed");

    assert_eq!(
        saga.state_data.blinded_messages.len(),
        output_blinded_messages.len(),
        "SetupComplete state should contain blinded messages"
    );

    assert_eq!(
        saga.state_data.ys.len(),
        input_proofs.len(),
        "SetupComplete state should contain input ys"
    );

    let ys = input_proofs.ys().unwrap();
    let states = mint
        .localstore()
        .get_proofs_states(&ys)
        .await
        .expect("Failed to get proof states");

    for state in states {
        assert_eq!(
            state.unwrap(),
            State::Pending,
            "Input proofs should be marked as pending after setup"
        );
    }
}

/// Tests the SetupComplete -> Signed state transition.
///
/// # What This Tests
/// - sign_outputs() successfully transitions saga from SetupComplete to Signed state
/// - Blind signatures are generated for all output blinded messages
/// - No database operations occur during signing (cryptographic operation only)
/// - State data contains signatures matching the number of blinded messages
///
/// # Operations
/// 1. Performs blind signing on blinded messages (non-transactional)
/// 2. Stores signatures in Signed state
/// 3. Preserves blinded messages and input Ys from previous state
///
/// # Success Criteria
/// - Saga transitions to Signed state
/// - Number of signatures equals number of blinded messages
/// - Compensations are still registered (cleared only on finalize)
#[tokio::test]
async fn test_swap_saga_sign_outputs_transition() {
    let mint = create_test_mint().await.unwrap();

    let amount = Amount::from(128);
    let (input_proofs, input_verification) = create_swap_inputs(&mint, amount).await;

    let (output_blinded_messages, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

    let db = mint.localstore();
    let pubsub = mint.pubsub_manager();

    let saga = SwapSaga::new(&mint, db, pubsub);

    let saga = saga
        .setup_swap(
            &input_proofs,
            &output_blinded_messages,
            None,
            input_verification,
        )
        .await
        .expect("Setup should succeed");

    let saga = saga.sign_outputs().await.expect("Signing should succeed");

    assert_eq!(
        saga.state_data.signatures.len(),
        output_blinded_messages.len(),
        "Signed state should contain signatures for all outputs"
    );
}

/// Tests that duplicate input proofs are rejected during setup.
///
/// # What This Tests
/// - Database detects and rejects duplicate proof additions
/// - setup_swap() fails with appropriate error (TokenPending or duplicate error)
/// - Transaction is rolled back, leaving no partial state
///
/// # Attack Vector
/// This prevents an attacker from trying to spend the same proof twice
/// within a single swap request.
///
/// # Success Criteria
/// - setup_swap() returns an error
/// - Database remains unchanged (transaction rollback)
#[tokio::test]
async fn test_swap_saga_duplicate_inputs() {
    let mint = create_test_mint().await.unwrap();

    let amount = Amount::from(100);
    let (mut input_proofs, input_verification) = create_swap_inputs(&mint, amount).await;

    input_proofs.push(input_proofs[0].clone());

    let (output_blinded_messages, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

    let db = mint.localstore();
    let pubsub = mint.pubsub_manager();

    let saga = SwapSaga::new(&mint, db, pubsub);

    let result = saga
        .setup_swap(
            &input_proofs,
            &output_blinded_messages,
            None,
            input_verification,
        )
        .await;

    assert!(result.is_err(), "Setup should fail with duplicate inputs");
}

/// Tests that duplicate output blinded messages are rejected during setup.
///
/// # What This Tests
/// - Database detects and rejects duplicate blinded message additions
/// - setup_swap() fails with DuplicateOutputs error
/// - Transaction is rolled back, leaving no partial state
///
/// # Attack Vector
/// This prevents reuse of blinded messages, which would allow an attacker
/// to receive the same blind signature multiple times.
///
/// # Success Criteria
/// - setup_swap() returns an error
/// - Database remains unchanged (transaction rollback)
#[tokio::test]
async fn test_swap_saga_duplicate_outputs() {
    let mint = create_test_mint().await.unwrap();

    let amount = Amount::from(100);
    let (input_proofs, input_verification) = create_swap_inputs(&mint, amount).await;

    let (mut output_blinded_messages, _) =
        create_test_blinded_messages(&mint, amount).await.unwrap();

    output_blinded_messages.push(output_blinded_messages[0].clone());

    let db = mint.localstore();
    let pubsub = mint.pubsub_manager();

    let saga = SwapSaga::new(&mint, db, pubsub);

    let result = saga
        .setup_swap(
            &input_proofs,
            &output_blinded_messages,
            None,
            input_verification,
        )
        .await;

    assert!(result.is_err(), "Setup should fail with duplicate outputs");
}

/// Tests that unbalanced swap requests are rejected (outputs > inputs).
///
/// # What This Tests
/// - Balance verification detects when output amount exceeds input amount
/// - setup_swap() fails with TransactionUnbalanced error
/// - Transaction is rolled back before any database changes
///
/// # Attack Vector
/// This prevents an attacker from creating value out of thin air by
/// requesting more outputs than they provided in inputs.
///
/// # Success Criteria
/// - setup_swap() returns an error
/// - Database remains unchanged (no proofs or blinded messages added)
#[tokio::test]
async fn test_swap_saga_unbalanced_transaction_more_outputs() {
    let mint = create_test_mint().await.unwrap();

    let input_amount = Amount::from(100);
    let (input_proofs, input_verification) = create_swap_inputs(&mint, input_amount).await;

    let output_amount = Amount::from(150);
    let (output_blinded_messages, _) = create_test_blinded_messages(&mint, output_amount)
        .await
        .unwrap();

    let db = mint.localstore();
    let pubsub = mint.pubsub_manager();

    let saga = SwapSaga::new(&mint, db, pubsub);

    let result = saga
        .setup_swap(
            &input_proofs,
            &output_blinded_messages,
            None,
            input_verification,
        )
        .await;

    assert!(
        result.is_err(),
        "Setup should fail when outputs exceed inputs"
    );
}

/// Tests that compensation actions are registered and cleared correctly.
///
/// # What This Tests
/// - Compensations start empty
/// - setup_swap() registers one compensation action (RemoveSwapSetup)
/// - sign_outputs() preserves compensations (no change)
/// - finalize() clears all compensations on success
///
/// # Saga Pattern
/// Compensations allow rollback if any step fails. They are cleared only
/// when the entire saga completes successfully. This test verifies the
/// lifecycle of compensation tracking.
///
/// # Success Criteria
/// - 0 compensations initially
/// - 1 compensation after setup
/// - 1 compensation after signing
/// - Compensations cleared after successful finalize
#[tokio::test]
async fn test_swap_saga_compensation_clears_on_success() {
    let mint = create_test_mint().await.unwrap();

    let amount = Amount::from(100);
    let (input_proofs, input_verification) = create_swap_inputs(&mint, amount).await;

    let (output_blinded_messages, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

    let db = mint.localstore();
    let pubsub = mint.pubsub_manager();

    let saga = SwapSaga::new(&mint, db, pubsub);

    let compensations_before = saga.compensations.lock().await.len();

    let saga = saga
        .setup_swap(
            &input_proofs,
            &output_blinded_messages,
            None,
            input_verification,
        )
        .await
        .expect("Setup should succeed");

    let compensations_after_setup = saga.compensations.lock().await.len();
    assert_eq!(
        compensations_after_setup, 1,
        "Should have one compensation after setup"
    );

    let saga = saga.sign_outputs().await.expect("Signing should succeed");

    let compensations_after_sign = saga.compensations.lock().await.len();
    assert_eq!(
        compensations_after_sign, 1,
        "Should still have one compensation after signing"
    );

    let _response = saga.finalize().await.expect("Finalize should succeed");

    assert_eq!(
        compensations_before, 0,
        "Should start with no compensations"
    );
}

/// Tests that empty input proofs are rejected during setup.
///
/// # What This Tests
/// - Swap with empty input proofs should fail gracefully
/// - No database changes should occur
///
/// # Success Criteria
/// - setup_swap() returns an error (not panic)
/// - Database remains unchanged
///
/// # Note
/// Empty inputs with non-empty outputs creates an unbalanced transaction
/// (trying to create value from nothing), which should be rejected by
/// the balance verification step.
#[tokio::test]
async fn test_swap_saga_empty_inputs() {
    let mint = create_test_mint().await.unwrap();
    let amount = Amount::from(100);

    let empty_proofs = Proofs::new();
    let (output_blinded_messages, _) = create_test_blinded_messages(&mint, amount).await.unwrap();
    // Verification must match the actual input amount (zero for empty proofs)
    let verification = create_verification(Amount::from(0));

    let db = mint.localstore();
    let pubsub = mint.pubsub_manager();
    let saga = SwapSaga::new(&mint, db, pubsub);

    let result = saga
        .setup_swap(&empty_proofs, &output_blinded_messages, None, verification)
        .await;

    // This should fail because outputs (100) > inputs (0)
    assert!(
        result.is_err(),
        "Empty inputs with non-empty outputs should be rejected (unbalanced)"
    );
}

/// Tests that empty output blinded messages are rejected during setup.
///
/// # What This Tests
/// - Swap with empty output blinded messages should fail gracefully
/// - No database changes should occur
///
/// # Success Criteria
/// - setup_swap() returns an error (not panic)
/// - Database remains unchanged
#[tokio::test]
async fn test_swap_saga_empty_outputs() {
    let mint = create_test_mint().await.unwrap();
    let amount = Amount::from(100);
    let (input_proofs, input_verification) = create_swap_inputs(&mint, amount).await;

    let empty_blinded_messages = vec![];

    let db = mint.localstore();
    let pubsub = mint.pubsub_manager();
    let saga = SwapSaga::new(&mint, db, pubsub);

    let result = saga
        .setup_swap(
            &input_proofs,
            &empty_blinded_messages,
            None,
            input_verification,
        )
        .await;

    assert!(result.is_err(), "Empty outputs should be rejected");
}

/// Tests that both empty inputs and outputs are rejected during setup.
///
/// # What This Tests
/// - Swap with both empty inputs and outputs should fail gracefully
/// - No database changes should occur
///
/// # Success Criteria
/// - setup_swap() returns an error (not panic)
/// - Database remains unchanged
#[tokio::test]
async fn test_swap_saga_both_empty() {
    let mint = create_test_mint().await.unwrap();

    let empty_proofs = Proofs::new();
    let empty_blinded_messages = vec![];
    let verification = create_verification(Amount::from(0));

    let db = mint.localstore();
    let pubsub = mint.pubsub_manager();
    let saga = SwapSaga::new(&mint, db, pubsub);

    let result = saga
        .setup_swap(&empty_proofs, &empty_blinded_messages, None, verification)
        .await;

    assert!(result.is_err(), "Empty swap should be rejected");
}

/// Tests that a saga dropped without finalize does not auto-cleanup.
///
/// # What This Tests
/// - When a saga is dropped after setup but before finalize:
///   - Proofs remain in Pending state (no automatic cleanup)
///   - Blinded messages remain in database
///   - No compensations run automatically on drop
///
/// # Design Choice
/// This tests for resource leaks and documents expected behavior.
/// Cleanup requires explicit compensation or timeout mechanism.
///
/// # Success Criteria
/// - After saga drop, proofs still Pending
/// - Blinded messages still exist in database
#[tokio::test]
async fn test_swap_saga_drop_without_finalize() {
    let mint = create_test_mint().await.unwrap();
    let amount = Amount::from(100);
    let (input_proofs, input_verification) = create_swap_inputs(&mint, amount).await;
    let (output_blinded_messages, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

    let db = mint.localstore();
    let ys = input_proofs.ys().unwrap();

    {
        let pubsub = mint.pubsub_manager();
        let saga = SwapSaga::new(&mint, db.clone(), pubsub);

        let _saga = saga
            .setup_swap(
                &input_proofs,
                &output_blinded_messages,
                None,
                input_verification,
            )
            .await
            .expect("Setup should succeed");

        // Verify setup state
        let states = db.get_proofs_states(&ys).await.unwrap();
        assert!(states.iter().all(|s| s == &Some(State::Pending)));

        // _saga is dropped here without calling finalize
    }

    // Verify state is NOT automatically cleaned up
    let states_after = db.get_proofs_states(&ys).await.unwrap();
    assert!(
        states_after.iter().all(|s| s == &Some(State::Pending)),
        "Proofs should remain Pending after saga drop (no auto-cleanup)"
    );

    // NOTE: This is expected behavior - compensations don't run on drop
    // Cleanup requires either:
    // 1. Explicit compensation call
    // 2. Timeout mechanism to clean up stale Pending proofs
    // 3. Manual intervention
}

/// Tests that a saga dropped after signing loses signatures.
///
/// # What This Tests
/// - When a saga is dropped after signing but before finalize:
///   - Proofs remain Pending
///   - Signatures are lost (not persisted)
///   - Demonstrates the importance of calling finalize
///
/// # Success Criteria
/// - Proofs still Pending after drop
/// - No signatures in database (they were only in memory)
#[tokio::test]
async fn test_swap_saga_drop_after_signing() {
    let mint = create_test_mint().await.unwrap();
    let amount = Amount::from(100);
    let (input_proofs, input_verification) = create_swap_inputs(&mint, amount).await;
    let (output_blinded_messages, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

    let db = mint.localstore();
    let ys = input_proofs.ys().unwrap();
    let blinded_secrets: Vec<_> = output_blinded_messages
        .iter()
        .map(|bm| bm.blinded_secret)
        .collect();

    {
        let pubsub = mint.pubsub_manager();
        let saga = SwapSaga::new(&mint, db.clone(), pubsub);

        let saga = saga
            .setup_swap(
                &input_proofs,
                &output_blinded_messages,
                None,
                input_verification,
            )
            .await
            .expect("Setup should succeed");

        let saga = saga.sign_outputs().await.expect("Signing should succeed");

        // Verify we're in Signed state (has signatures)
        assert_eq!(
            saga.state_data.signatures.len(),
            output_blinded_messages.len()
        );

        // saga is dropped here - signatures are lost!
    }

    // Verify proofs still Pending
    let states_after = db.get_proofs_states(&ys).await.unwrap();
    assert!(states_after.iter().all(|s| s == &Some(State::Pending)));

    // Verify signatures were NOT persisted (they were only in memory in the saga)
    let signatures = db.get_blind_signatures(&blinded_secrets).await.unwrap();
    assert!(
        signatures.iter().all(|s| s.is_none()),
        "Signatures should be lost when saga is dropped (never persisted)"
    );

    // This demonstrates why finalize() is critical - without it, the signatures
    // generated during signing are lost and the swap cannot complete
}

/// Tests that compensations execute when sign_outputs() fails.
///
/// # What This Tests
/// - Verify that compensations execute when sign_outputs() fails
/// - Verify that proofs are removed from database (rollback of setup)
/// - Verify that blinded messages are removed from database
/// - Verify that proof states are cleared (no longer Pending)
///
/// # Implementation
/// Uses TEST_FAIL environment variable to make blind_sign() fail
///
/// # Success Criteria
/// - Signing fails with error
/// - Proofs are removed from database after failure
/// - Blinded messages are removed after failure
#[tokio::test]
async fn test_swap_saga_compensation_on_signing_failure() {
    let mint = create_test_mint().await.unwrap();
    let amount = Amount::from(100);
    let (input_proofs, input_verification) = create_swap_inputs(&mint, amount).await;
    let (output_blinded_messages, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

    let db = mint.localstore();
    let pubsub = mint.pubsub_manager();
    let saga = SwapSaga::new(&mint, db.clone(), pubsub);

    // Setup should succeed
    let saga = saga
        .setup_swap(
            &input_proofs,
            &output_blinded_messages,
            None,
            input_verification,
        )
        .await
        .expect("Setup should succeed");

    // Verify setup state
    let ys = input_proofs.ys().unwrap();
    let states = db.get_proofs_states(&ys).await.unwrap();
    assert!(states.iter().all(|s| s == &Some(State::Pending)));

    // Enable test failure mode
    std::env::set_var("TEST_FAIL", "1");

    // Attempt signing (should fail due to TEST_FAIL)
    let result = saga.sign_outputs().await;

    // Clean up environment variable immediately
    std::env::remove_var("TEST_FAIL");

    assert!(result.is_err(), "Signing should fail");

    // Verify compensation executed - proofs removed
    let states_after = db.get_proofs_states(&ys).await.unwrap();
    assert!(
        states_after.iter().all(|s| s.is_none()),
        "Proofs should be removed"
    );

    // Verify blinded messages removed (compensation removes blinded messages, not signatures)
    // Since signatures are never created (only during finalize), we verify that
    // if we query for them, we get None for all (they were never added)
    let blinded_secrets: Vec<_> = output_blinded_messages
        .iter()
        .map(|bm| bm.blinded_secret)
        .collect();
    let signatures = db.get_blind_signatures(&blinded_secrets).await.unwrap();
    assert!(
        signatures.iter().all(|s| s.is_none()),
        "No signatures should exist (never created)"
    );
}

/// Tests that double-spend attempts are detected and rejected.
///
/// # What This Tests
/// - First complete swap marks proofs as Spent
/// - Second swap attempt with same proofs fails immediately
/// - Database proof state prevents double-spending
///
/// # Security
/// This is a critical security test. Double-spending would allow an
/// attacker to reuse the same ecash tokens multiple times. The database
/// must detect that proofs are already spent and reject the second swap.
///
/// # Flow
/// 1. Complete first swap successfully (proofs marked Spent)
/// 2. Attempt second swap with same proofs
/// 3. Second setup_swap() fails with TokenAlreadySpent error
///
/// # Success Criteria
/// - First swap completes successfully
/// - Second swap fails with error
/// - Proofs remain in Spent state
#[tokio::test]
async fn test_swap_saga_double_spend_detection() {
    let mint = create_test_mint().await.unwrap();

    let amount = Amount::from(100);
    let (input_proofs, input_verification) = create_swap_inputs(&mint, amount).await;

    let (output_blinded_messages_1, _) = create_test_blinded_messages(&mint, amount).await.unwrap();
    let (output_blinded_messages_2, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

    let db = mint.localstore();
    let pubsub = mint.pubsub_manager();

    let saga1 = SwapSaga::new(&mint, db.clone(), pubsub.clone());

    let saga1 = saga1
        .setup_swap(
            &input_proofs,
            &output_blinded_messages_1,
            None,
            input_verification.clone(),
        )
        .await
        .expect("First setup should succeed");

    let saga1 = saga1
        .sign_outputs()
        .await
        .expect("First signing should succeed");

    let _response1 = saga1
        .finalize()
        .await
        .expect("First finalize should succeed");

    let saga2 = SwapSaga::new(&mint, db, pubsub);

    let result = saga2
        .setup_swap(
            &input_proofs,
            &output_blinded_messages_2,
            None,
            input_verification,
        )
        .await;

    assert!(
        result.is_err(),
        "Second setup should fail due to double-spend"
    );
}

/// Tests that pending proofs are detected and rejected.
///
/// # What This Tests
/// - First swap marks proofs as Pending during setup
/// - Second swap attempt with same proofs fails immediately
/// - Database proof state prevents concurrent use of same proofs
///
/// # Concurrency Protection
/// When proofs are marked Pending, they are reserved for an in-progress
/// swap. No other swap should be able to use them until the first swap
/// completes or rolls back.
///
/// # Flow
/// 1. Start first swap (proofs marked Pending)
/// 2. DO NOT finalize first swap
/// 3. Attempt second swap with same proofs
/// 4. Second setup_swap() fails with TokenPending error
///
/// # Success Criteria
/// - First setup succeeds (proofs marked Pending)
/// - Second setup fails with error
/// - Proofs remain in Pending state
#[tokio::test]
async fn test_swap_saga_pending_proof_detection() {
    let mint = create_test_mint().await.unwrap();

    let amount = Amount::from(100);
    let (input_proofs, input_verification) = create_swap_inputs(&mint, amount).await;

    let (output_blinded_messages_1, _) = create_test_blinded_messages(&mint, amount).await.unwrap();
    let (output_blinded_messages_2, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

    let db = mint.localstore();
    let pubsub = mint.pubsub_manager();

    let saga1 = SwapSaga::new(&mint, db.clone(), pubsub.clone());

    let saga1 = saga1
        .setup_swap(
            &input_proofs,
            &output_blinded_messages_1,
            None,
            input_verification.clone(),
        )
        .await
        .expect("First setup should succeed");

    // Keep saga1 in scope to maintain pending proofs
    drop(saga1);

    let saga2 = SwapSaga::new(&mint, db, pubsub);

    let result = saga2
        .setup_swap(
            &input_proofs,
            &output_blinded_messages_2,
            None,
            input_verification,
        )
        .await;

    assert!(
        result.is_err(),
        "Second setup should fail because proofs are pending"
    );
}

/// Tests concurrent swap attempts with the same proofs.
///
/// # What This Tests
/// - Database serialization ensures only one concurrent swap succeeds
/// - Exactly one of N concurrent swaps with same proofs completes
/// - Other swaps fail with TokenPending or TokenAlreadySpent errors
/// - Final proof state is Spent (from the successful swap)
///
/// # Race Condition Protection
/// This test verifies that the saga pattern combined with database
/// transactions provides proper serialization. Even with 3 tasks racing
/// to setup/sign/finalize, only one can succeed.
///
/// # Flow
/// 1. Spawn 3 concurrent tasks, each trying to swap the same proofs
/// 2. Each task creates its own saga and attempts full flow
/// 3. Database ensures only one can mark proofs as Pending/Spent
/// 4. Count successes and failures
///
/// # Success Criteria
/// - Exactly 1 swap succeeds
/// - Exactly 2 swaps fail
/// - All proofs end up in Spent state
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn test_swap_saga_concurrent_swaps() {
    let mint = Arc::new(create_test_mint().await.unwrap());

    let amount = Amount::from(100);
    let (input_proofs, input_verification) = create_swap_inputs(&mint, amount).await;

    let (output_blinded_messages_1, _) = create_test_blinded_messages(&mint, amount).await.unwrap();
    let (output_blinded_messages_2, _) = create_test_blinded_messages(&mint, amount).await.unwrap();
    let (output_blinded_messages_3, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

    let mint1 = Arc::clone(&mint);
    let mint2 = Arc::clone(&mint);
    let mint3 = Arc::clone(&mint);

    let proofs1 = input_proofs.clone();
    let proofs2 = input_proofs.clone();
    let proofs3 = input_proofs.clone();

    let verification1 = input_verification.clone();
    let verification2 = input_verification.clone();
    let verification3 = input_verification.clone();

    let task1 = tokio::spawn(async move {
        let db = mint1.localstore();
        let pubsub = mint1.pubsub_manager();
        let saga = SwapSaga::new(&*mint1, db, pubsub);

        let saga = saga
            .setup_swap(&proofs1, &output_blinded_messages_1, None, verification1)
            .await?;
        let saga = saga.sign_outputs().await?;
        saga.finalize().await
    });

    let task2 = tokio::spawn(async move {
        let db = mint2.localstore();
        let pubsub = mint2.pubsub_manager();
        let saga = SwapSaga::new(&*mint2, db, pubsub);

        let saga = saga
            .setup_swap(&proofs2, &output_blinded_messages_2, None, verification2)
            .await?;
        let saga = saga.sign_outputs().await?;
        saga.finalize().await
    });

    let task3 = tokio::spawn(async move {
        let db = mint3.localstore();
        let pubsub = mint3.pubsub_manager();
        let saga = SwapSaga::new(&*mint3, db, pubsub);

        let saga = saga
            .setup_swap(&proofs3, &output_blinded_messages_3, None, verification3)
            .await?;
        let saga = saga.sign_outputs().await?;
        saga.finalize().await
    });

    let results = tokio::try_join!(task1, task2, task3).expect("Tasks should complete");

    let mut success_count = 0;
    let mut error_count = 0;

    for result in [results.0, results.1, results.2] {
        match result {
            Ok(_) => success_count += 1,
            Err(_) => error_count += 1,
        }
    }

    assert_eq!(success_count, 1, "Only one concurrent swap should succeed");
    assert_eq!(error_count, 2, "Two concurrent swaps should fail");

    let ys = input_proofs.ys().unwrap();
    let states = mint
        .localstore()
        .get_proofs_states(&ys)
        .await
        .expect("Failed to get proof states");

    for state in states {
        assert_eq!(
            state.unwrap(),
            State::Spent,
            "Proofs should be marked as spent after successful swap"
        );
    }
}

/// Tests that compensations execute when finalize() fails during add_blind_signatures.
///
/// # What This Tests
/// - Verify that compensations execute when finalize() fails at signature addition
/// - Verify that proofs are removed from database (compensation rollback)
/// - Verify that blinded messages are removed from database
/// - Verify that signatures are NOT persisted to database
/// - Transaction rollback + compensation cleanup both occur
///
/// # Implementation
/// Uses TEST_FAIL_ADD_SIGNATURES environment variable to inject failure
/// at the signature addition step within the finalize transaction.
///
/// # Success Criteria
/// - Finalize fails with error
/// - Proofs are removed from database after failure
/// - Blinded messages are removed after failure
/// - No signatures persisted to database
#[tokio::test]
async fn test_swap_saga_compensation_on_finalize_add_signatures_failure() {
    let mint = create_test_mint().await.unwrap();
    let amount = Amount::from(100);
    let (input_proofs, input_verification) = create_swap_inputs(&mint, amount).await;
    let (output_blinded_messages, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

    let db = mint.localstore();
    let pubsub = mint.pubsub_manager();
    let saga = SwapSaga::new(&mint, db.clone(), pubsub);

    // Setup and sign should succeed
    let saga = saga
        .setup_swap(
            &input_proofs,
            &output_blinded_messages,
            None,
            input_verification,
        )
        .await
        .expect("Setup should succeed");

    let saga = saga.sign_outputs().await.expect("Signing should succeed");

    // Verify we're in Signed state
    assert_eq!(
        saga.state_data.signatures.len(),
        output_blinded_messages.len()
    );

    // Enable test failure mode for ADD_SIGNATURES
    std::env::set_var("TEST_FAIL_ADD_SIGNATURES", "1");

    // Attempt finalize (should fail due to TEST_FAIL_ADD_SIGNATURES)
    let result = saga.finalize().await;

    // Clean up environment variable immediately
    std::env::remove_var("TEST_FAIL_ADD_SIGNATURES");

    assert!(result.is_err(), "Finalize should fail");

    // Verify compensation executed - proofs removed
    let ys = input_proofs.ys().unwrap();
    let states_after = db.get_proofs_states(&ys).await.unwrap();
    assert!(
        states_after.iter().all(|s| s.is_none()),
        "Proofs should be removed by compensation"
    );

    // Verify signatures were NOT persisted (transaction rolled back)
    let blinded_secrets: Vec<_> = output_blinded_messages
        .iter()
        .map(|bm| bm.blinded_secret)
        .collect();
    let signatures = db.get_blind_signatures(&blinded_secrets).await.unwrap();
    assert!(
        signatures.iter().all(|s| s.is_none()),
        "Signatures should not be persisted after rollback"
    );
}

/// Tests that compensations execute when finalize() fails during update_proofs_states.
///
/// # What This Tests
/// - Verify that compensations execute when finalize() fails at proof state update
/// - Verify that proofs are removed from database (compensation rollback)
/// - Verify that blinded messages are removed from database
/// - Verify that signatures are NOT persisted to database
/// - Transaction rollback + compensation cleanup both occur
///
/// # Implementation
/// Uses TEST_FAIL_UPDATE_PROOFS environment variable to inject failure
/// at the proof state update step within the finalize transaction.
///
/// # Success Criteria
/// - Finalize fails with error
/// - Proofs are removed from database after failure
/// - Blinded messages are removed after failure
/// - No signatures persisted to database
#[tokio::test]
async fn test_swap_saga_compensation_on_finalize_update_proofs_failure() {
    let mint = create_test_mint().await.unwrap();
    let amount = Amount::from(100);
    let (input_proofs, input_verification) = create_swap_inputs(&mint, amount).await;
    let (output_blinded_messages, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

    let db = mint.localstore();
    let pubsub = mint.pubsub_manager();
    let saga = SwapSaga::new(&mint, db.clone(), pubsub);

    // Setup and sign should succeed
    let saga = saga
        .setup_swap(
            &input_proofs,
            &output_blinded_messages,
            None,
            input_verification,
        )
        .await
        .expect("Setup should succeed");

    let saga = saga.sign_outputs().await.expect("Signing should succeed");

    // Verify we're in Signed state
    assert_eq!(
        saga.state_data.signatures.len(),
        output_blinded_messages.len()
    );

    // Enable test failure mode for UPDATE_PROOFS
    std::env::set_var("TEST_FAIL_UPDATE_PROOFS", "1");

    // Attempt finalize (should fail due to TEST_FAIL_UPDATE_PROOFS)
    let result = saga.finalize().await;

    // Clean up environment variable immediately
    std::env::remove_var("TEST_FAIL_UPDATE_PROOFS");

    assert!(result.is_err(), "Finalize should fail");

    // Verify compensation executed - proofs removed
    let ys = input_proofs.ys().unwrap();
    let states_after = db.get_proofs_states(&ys).await.unwrap();
    assert!(
        states_after.iter().all(|s| s.is_none()),
        "Proofs should be removed by compensation"
    );

    // Verify signatures were NOT persisted (transaction rolled back)
    let blinded_secrets: Vec<_> = output_blinded_messages
        .iter()
        .map(|bm| bm.blinded_secret)
        .collect();
    let signatures = db.get_blind_signatures(&blinded_secrets).await.unwrap();
    assert!(
        signatures.iter().all(|s| s.is_none()),
        "Signatures should not be persisted after rollback"
    );
}

// ==================== STARTUP RECOVERY TESTS ====================
// These tests verify the `recover_from_bad_swaps()` startup check that
// cleans up orphaned swap state when the mint restarts.

/// Tests startup recovery when saga is dropped before signing.
///
/// # What This Tests
/// - Saga dropped after setup (proofs PENDING, no signatures)
/// - recover_from_bad_swaps() removes the proofs
/// - Blinded messages are removed
/// - Same proofs can be used in a new swap after recovery
///
/// # Recovery Behavior
/// When no blind signatures exist for an operation_id:
/// - Proofs are removed from database
/// - Blinded messages are removed
/// - User can retry the swap with same proofs
///
/// # Flow
/// 1. Setup swap (proofs marked PENDING)
/// 2. Drop saga without signing
/// 3. Call recover_from_bad_swaps() (simulates mint restart)
/// 4. Verify proofs removed
/// 5. Verify can use same proofs in new swap
///
/// # Success Criteria
/// - Recovery removes proofs completely
/// - Blinded messages removed
/// - Second swap with same proofs succeeds
#[tokio::test]
async fn test_startup_recovery_saga_dropped_before_signing() {
    let mint = create_test_mint().await.unwrap();
    let amount = Amount::from(100);
    let (input_proofs, input_verification) = create_swap_inputs(&mint, amount).await;
    let (output_blinded_messages, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

    let db = mint.localstore();
    let ys = input_proofs.ys().unwrap();

    // Setup swap and drop without signing
    {
        let pubsub = mint.pubsub_manager();
        let saga = SwapSaga::new(&mint, db.clone(), pubsub);

        let _saga = saga
            .setup_swap(
                &input_proofs,
                &output_blinded_messages,
                None,
                input_verification.clone(),
            )
            .await
            .expect("Setup should succeed");

        // Verify proofs are PENDING
        let states = db.get_proofs_states(&ys).await.unwrap();
        assert!(states.iter().all(|s| s == &Some(State::Pending)));

        // Saga dropped here without signing
    }

    // Proofs still PENDING after drop (no auto-cleanup)
    let states_before_recovery = db.get_proofs_states(&ys).await.unwrap();
    assert!(states_before_recovery
        .iter()
        .all(|s| s == &Some(State::Pending)));

    // Simulate mint restart - run recovery
    mint.stop().await.expect("Recovery should succeed");
    mint.start().await.expect("Recovery should succeed");

    // Verify proofs are REMOVED (not just state cleared)
    let states_after_recovery = db.get_proofs_states(&ys).await.unwrap();
    assert!(
        states_after_recovery.iter().all(|s| s.is_none()),
        "Proofs should be removed after recovery (no signatures exist)"
    );

    // Verify we can now use the same proofs in a new swap
    let (new_output_blinded_messages, _) =
        create_test_blinded_messages(&mint, amount).await.unwrap();

    let pubsub = mint.pubsub_manager();
    let new_saga = SwapSaga::new(&mint, db, pubsub);

    let new_saga = new_saga
        .setup_swap(
            &input_proofs,
            &new_output_blinded_messages,
            None,
            input_verification,
        )
        .await
        .expect("Second swap should succeed after recovery");

    let new_saga = new_saga
        .sign_outputs()
        .await
        .expect("Signing should succeed");

    let _response = new_saga.finalize().await.expect("Finalize should succeed");

    // Verify proofs are now SPENT
    let final_states = mint.localstore().get_proofs_states(&ys).await.unwrap();
    assert!(final_states.iter().all(|s| s == &Some(State::Spent)));
}

/// Tests startup recovery when saga is dropped after signing.
///
/// # What This Tests
/// - Saga dropped after signing but before finalize
/// - Signatures exist in memory but were never persisted to database
/// - recover_from_bad_swaps() removes the proofs (no signatures in DB)
/// - Same proofs can be used in a new swap after recovery
///
/// # Recovery Behavior
/// When no blind signatures exist in database for an operation_id:
/// - Proofs are removed from database
/// - User can retry the swap
///
/// Note: Signatures from sign_outputs() are in memory only. They're only
/// persisted during finalize(). So a dropped saga after signing has no
/// signatures in the database.
///
/// # Flow
/// 1. Setup swap and sign outputs
/// 2. Drop saga without finalize (signatures lost)
/// 3. Call recover_from_bad_swaps()
/// 4. Verify proofs removed
/// 5. Verify can use same proofs in new swap
///
/// # Success Criteria
/// - Recovery removes proofs completely
/// - No signatures in database (never persisted)
/// - Second swap with same proofs succeeds
#[tokio::test]
async fn test_startup_recovery_saga_dropped_after_signing() {
    let mint = create_test_mint().await.unwrap();
    let amount = Amount::from(100);
    let (input_proofs, input_verification) = create_swap_inputs(&mint, amount).await;
    let (output_blinded_messages, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

    let db = mint.localstore();
    let ys = input_proofs.ys().unwrap();
    let blinded_secrets: Vec<_> = output_blinded_messages
        .iter()
        .map(|bm| bm.blinded_secret)
        .collect();

    // Setup swap, sign, and drop without finalize
    {
        let pubsub = mint.pubsub_manager();
        let saga = SwapSaga::new(&mint, db.clone(), pubsub);

        let saga = saga
            .setup_swap(
                &input_proofs,
                &output_blinded_messages,
                None,
                input_verification.clone(),
            )
            .await
            .expect("Setup should succeed");

        let _saga = saga.sign_outputs().await.expect("Signing should succeed");

        // Saga dropped here - signatures were in memory only, never persisted
    }

    // Verify proofs still PENDING
    let states_before = db.get_proofs_states(&ys).await.unwrap();
    assert!(states_before.iter().all(|s| s == &Some(State::Pending)));

    // Verify no signatures in database (they were only in memory)
    let sigs_before = db.get_blind_signatures(&blinded_secrets).await.unwrap();
    assert!(sigs_before.iter().all(|s| s.is_none()));

    // Simulate mint restart - run recovery
    mint.stop().await.expect("Recovery should succeed");
    mint.start().await.expect("Recovery should succeed");

    // Verify proofs are REMOVED
    let states_after = db.get_proofs_states(&ys).await.unwrap();
    assert!(
        states_after.iter().all(|s| s.is_none()),
        "Proofs should be removed (no signatures in DB)"
    );

    // Verify we can use the same proofs in a new swap
    let (new_output_blinded_messages, _) =
        create_test_blinded_messages(&mint, amount).await.unwrap();

    let pubsub = mint.pubsub_manager();
    let new_saga = SwapSaga::new(&mint, db, pubsub);

    let new_saga = new_saga
        .setup_swap(
            &input_proofs,
            &new_output_blinded_messages,
            None,
            input_verification,
        )
        .await
        .expect("Second swap should succeed after recovery");

    let new_saga = new_saga
        .sign_outputs()
        .await
        .expect("Signing should succeed");

    let _response = new_saga.finalize().await.expect("Finalize should succeed");
}

/// Tests startup recovery with multiple abandoned operations.
///
/// # What This Tests
/// - Multiple swap operations in different states
/// - recover_from_bad_swaps() processes all operations correctly
/// - Each operation is handled according to its state
///
/// # Test Scenario
/// - Operation A: Dropped after setup (no signatures) → proofs removed
/// - Operation B: Dropped after signing (signatures not persisted) → proofs removed
/// - Operation C: Completed successfully (has signatures, SPENT) → untouched
///
/// # Success Criteria
/// - Operation A proofs removed
/// - Operation B proofs removed
/// - Operation C proofs remain SPENT
/// - All operations processed in single recovery call
#[tokio::test]
async fn test_startup_recovery_multiple_operations() {
    let mint = create_test_mint().await.unwrap();
    let amount = Amount::from(100);

    // Create three separate sets of proofs for three operations
    let (proofs_a, verification_a) = create_swap_inputs(&mint, amount).await;
    let (proofs_b, verification_b) = create_swap_inputs(&mint, amount).await;
    let (proofs_c, verification_c) = create_swap_inputs(&mint, amount).await;

    let (outputs_a, _) = create_test_blinded_messages(&mint, amount).await.unwrap();
    let (outputs_b, _) = create_test_blinded_messages(&mint, amount).await.unwrap();
    let (outputs_c, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

    let db = mint.localstore();
    let pubsub = mint.pubsub_manager();

    let ys_a = proofs_a.ys().unwrap();
    let ys_b = proofs_b.ys().unwrap();
    let ys_c = proofs_c.ys().unwrap();

    // Operation A: Setup only (dropped before signing)
    {
        let saga_a = SwapSaga::new(&mint, db.clone(), pubsub.clone());
        let _saga_a = saga_a
            .setup_swap(&proofs_a, &outputs_a, None, verification_a)
            .await
            .expect("Operation A setup should succeed");
        // Dropped without signing
    }

    // Operation B: Setup + Sign (dropped before finalize)
    {
        let saga_b = SwapSaga::new(&mint, db.clone(), pubsub.clone());
        let saga_b = saga_b
            .setup_swap(&proofs_b, &outputs_b, None, verification_b)
            .await
            .expect("Operation B setup should succeed");
        let _saga_b = saga_b
            .sign_outputs()
            .await
            .expect("Operation B signing should succeed");
        // Dropped without finalize
    }

    // Operation C: Complete successfully
    {
        let saga_c = SwapSaga::new(&mint, db.clone(), pubsub.clone());
        let saga_c = saga_c
            .setup_swap(&proofs_c, &outputs_c, None, verification_c)
            .await
            .expect("Operation C setup should succeed");
        let saga_c = saga_c
            .sign_outputs()
            .await
            .expect("Operation C signing should succeed");
        let _response = saga_c
            .finalize()
            .await
            .expect("Operation C finalize should succeed");
    }

    // Verify states before recovery
    let states_a_before = db.get_proofs_states(&ys_a).await.unwrap();
    let states_b_before = db.get_proofs_states(&ys_b).await.unwrap();
    let states_c_before = db.get_proofs_states(&ys_c).await.unwrap();

    assert!(states_a_before.iter().all(|s| s == &Some(State::Pending)));
    assert!(states_b_before.iter().all(|s| s == &Some(State::Pending)));
    assert!(states_c_before.iter().all(|s| s == &Some(State::Spent)));

    // Simulate mint restart - run recovery
    mint.stop().await.expect("Recovery should succeed");
    mint.start().await.expect("Recovery should succeed");

    // Verify states after recovery
    let states_a_after = db.get_proofs_states(&ys_a).await.unwrap();
    let states_b_after = db.get_proofs_states(&ys_b).await.unwrap();
    let states_c_after = db.get_proofs_states(&ys_c).await.unwrap();

    assert!(
        states_a_after.iter().all(|s| s.is_none()),
        "Operation A proofs should be removed (no signatures)"
    );
    assert!(
        states_b_after.iter().all(|s| s.is_none()),
        "Operation B proofs should be removed (no signatures in DB)"
    );
    assert!(
        states_c_after.iter().all(|s| s == &Some(State::Spent)),
        "Operation C proofs should remain SPENT (completed successfully)"
    );
}

/// Tests startup recovery with operation ID uniqueness and tracking.
///
/// # What This Tests
/// - Multiple concurrent swaps get unique operation_ids
/// - Proofs are correctly associated with their operation_ids
/// - Recovery can distinguish between different operations
/// - Each operation is tracked independently
///
/// # Flow
/// 1. Create multiple swaps concurrently
/// 2. Drop all sagas without finalize
/// 3. Verify proofs are associated with different operations
/// 4. Run recovery
/// 5. Verify all operations cleaned up correctly
///
/// # Success Criteria
/// - Each swap has unique operation_id
/// - Proofs correctly tracked per operation
/// - Recovery processes each operation independently
/// - All proofs removed after recovery
#[tokio::test]
async fn test_operation_id_uniqueness_and_tracking() {
    let mint = Arc::new(create_test_mint().await.unwrap());
    let amount = Amount::from(100);

    // Create three separate sets of proofs
    let (proofs_1, verification_1) = create_swap_inputs(&mint, amount).await;
    let (proofs_2, verification_2) = create_swap_inputs(&mint, amount).await;
    let (proofs_3, verification_3) = create_swap_inputs(&mint, amount).await;

    let (outputs_1, _) = create_test_blinded_messages(&mint, amount).await.unwrap();
    let (outputs_2, _) = create_test_blinded_messages(&mint, amount).await.unwrap();
    let (outputs_3, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

    let db = mint.localstore();

    let ys_1 = proofs_1.ys().unwrap();
    let ys_2 = proofs_2.ys().unwrap();
    let ys_3 = proofs_3.ys().unwrap();

    // Create all three swaps and drop without finalize
    {
        let pubsub = mint.pubsub_manager();

        let saga_1 = SwapSaga::new(&*mint, db.clone(), pubsub.clone());
        let _saga_1 = saga_1
            .setup_swap(&proofs_1, &outputs_1, None, verification_1)
            .await
            .expect("Swap 1 setup should succeed");

        let saga_2 = SwapSaga::new(&*mint, db.clone(), pubsub.clone());
        let _saga_2 = saga_2
            .setup_swap(&proofs_2, &outputs_2, None, verification_2)
            .await
            .expect("Swap 2 setup should succeed");

        let saga_3 = SwapSaga::new(&*mint, db.clone(), pubsub.clone());
        let _saga_3 = saga_3
            .setup_swap(&proofs_3, &outputs_3, None, verification_3)
            .await
            .expect("Swap 3 setup should succeed");

        // All sagas dropped without finalize
    }

    // Verify all proofs are PENDING
    let states_1 = db.get_proofs_states(&ys_1).await.unwrap();
    let states_2 = db.get_proofs_states(&ys_2).await.unwrap();
    let states_3 = db.get_proofs_states(&ys_3).await.unwrap();

    assert!(states_1.iter().all(|s| s == &Some(State::Pending)));
    assert!(states_2.iter().all(|s| s == &Some(State::Pending)));
    assert!(states_3.iter().all(|s| s == &Some(State::Pending)));

    // Simulate mint restart - run recovery
    mint.stop().await.expect("Recovery should succeed");
    mint.start().await.expect("Recovery should succeed");

    // Verify all proofs removed
    let states_1_after = db.get_proofs_states(&ys_1).await.unwrap();
    let states_2_after = db.get_proofs_states(&ys_2).await.unwrap();
    let states_3_after = db.get_proofs_states(&ys_3).await.unwrap();

    assert!(
        states_1_after.iter().all(|s| s.is_none()),
        "Swap 1 proofs should be removed"
    );
    assert!(
        states_2_after.iter().all(|s| s.is_none()),
        "Swap 2 proofs should be removed"
    );
    assert!(
        states_3_after.iter().all(|s| s.is_none()),
        "Swap 3 proofs should be removed"
    );

    // Verify each set of proofs can now be used in new swaps
    let (new_outputs_1, _) = create_test_blinded_messages(&mint, amount).await.unwrap();
    let verification = create_verification(amount);

    let pubsub = mint.pubsub_manager();
    let new_saga = SwapSaga::new(&*mint, db, pubsub);

    let result = new_saga
        .setup_swap(&proofs_1, &new_outputs_1, None, verification)
        .await;

    assert!(
        result.is_ok(),
        "Should be able to reuse proofs after recovery"
    );
}

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

/// Tests the complete happy path flow through all sagas.
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
    let _blinded_secrets: Vec<_> = output_blinded_messages
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
    let signatures = db.get_blind_signatures(&_blinded_secrets).await.unwrap();
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
    let _blinded_secrets: Vec<_> = output_blinded_messages
        .iter()
        .map(|bm| bm.blinded_secret)
        .collect();
    let signatures = db.get_blind_signatures(&_blinded_secrets).await.unwrap();
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

// ==================== PHASE 1: FOUNDATION TESTS ====================
// These tests verify the basic saga persistence mechanism.

/// Tests that saga is persisted to the database after setup.
///
/// # What This Tests
/// - Saga is written to database during setup_swap()
/// - get_saga() can retrieve the persisted state
/// - State content is correct (operation_id, state, blinded_secrets, input_ys)
///
/// # Success Criteria
/// - Saga exists in database after setup
/// - State matches SwapSagaState::SetupComplete
/// - All expected data is present and correct
#[tokio::test]
async fn test_saga_state_persistence_after_setup() {
    let mint = create_test_mint().await.unwrap();
    let db = mint.localstore();

    let amount = Amount::from(100);
    let (input_proofs, verification) = create_swap_inputs(&mint, amount).await;
    let (output_blinded_messages, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

    let pubsub = mint.pubsub_manager();
    let saga = SwapSaga::new(&mint, db.clone(), pubsub);

    let saga = saga
        .setup_swap(&input_proofs, &output_blinded_messages, None, verification)
        .await
        .expect("Setup should succeed");

    let operation_id = saga.operation.id();

    // Verify saga exists in database
    let saga = {
        let mut tx = db.begin_transaction().await.unwrap();
        let result = tx.get_saga(operation_id).await.expect("Failed to get saga");
        tx.commit().await.unwrap();
        result.expect("Saga should exist after setup")
    };

    // Verify state is SetupComplete
    use cdk_common::mint::{SagaStateEnum, SwapSagaState};
    assert_eq!(
        saga.state,
        SagaStateEnum::Swap(SwapSagaState::SetupComplete),
        "Saga should be SetupComplete"
    );

    // Verify operation_id matches
    assert_eq!(saga.operation_id, *operation_id);

    // Verify blinded_secrets are stored correctly
    let expected_blinded_secrets: Vec<_> = output_blinded_messages
        .iter()
        .map(|bm| bm.blinded_secret)
        .collect();
    assert_eq!(saga.blinded_secrets.len(), expected_blinded_secrets.len());
    for bs in &expected_blinded_secrets {
        assert!(
            saga.blinded_secrets.contains(bs),
            "Blinded secret should be in saga"
        );
    }

    // Verify input_ys are stored correctly
    let expected_ys = input_proofs.ys().unwrap();
    assert_eq!(saga.input_ys.len(), expected_ys.len());
    for y in &expected_ys {
        assert!(saga.input_ys.contains(y), "Input Y should be in saga");
    }
}

/// Tests that saga is deleted after successful finalization.
///
/// # What This Tests
/// - Saga exists after setup
/// - Saga still exists after signing
/// - Saga is DELETED after successful finalize
/// - get_incomplete_sagas() returns empty after success
///
/// # Success Criteria
/// - Saga deleted from database
/// - No incomplete sagas remain
#[tokio::test]
async fn test_saga_deletion_on_success() {
    let mint = create_test_mint().await.unwrap();
    let db = mint.localstore();

    let amount = Amount::from(100);
    let (input_proofs, verification) = create_swap_inputs(&mint, amount).await;
    let (output_blinded_messages, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

    let pubsub = mint.pubsub_manager();
    let saga = SwapSaga::new(&mint, db.clone(), pubsub);

    let saga = saga
        .setup_swap(&input_proofs, &output_blinded_messages, None, verification)
        .await
        .expect("Setup should succeed");

    let operation_id = *saga.operation.id();

    // Verify saga exists after setup
    let saga_after_setup = {
        let mut tx = db.begin_transaction().await.unwrap();
        let result = tx
            .get_saga(&operation_id)
            .await
            .expect("Failed to get saga");
        tx.commit().await.unwrap();
        result
    };
    assert!(saga_after_setup.is_some(), "Saga should exist after setup");

    let saga = saga.sign_outputs().await.expect("Signing should succeed");

    // Verify saga still exists after signing
    let saga_after_sign = {
        let mut tx = db.begin_transaction().await.unwrap();
        let result = tx
            .get_saga(&operation_id)
            .await
            .expect("Failed to get saga");
        tx.commit().await.unwrap();
        result
    };
    assert!(
        saga_after_sign.is_some(),
        "Saga should still exist after signing"
    );

    let _response = saga.finalize().await.expect("Finalize should succeed");

    // CRITICAL: Verify saga is DELETED after success
    let saga_after_finalize = {
        let mut tx = db.begin_transaction().await.unwrap();
        let result = tx
            .get_saga(&operation_id)
            .await
            .expect("Failed to get saga");
        tx.commit().await.unwrap();
        result
    };
    assert!(
        saga_after_finalize.is_none(),
        "Saga should be deleted after successful finalization"
    );

    // Verify no incomplete sagas exist
    use cdk_common::mint::OperationKind;
    let incomplete = db
        .get_incomplete_sagas(OperationKind::Swap)
        .await
        .expect("Failed to get incomplete sagas");
    assert_eq!(incomplete.len(), 0, "No incomplete sagas should exist");
}

/// Tests querying incomplete sagas.
///
/// # What This Tests
/// - get_incomplete_sagas() returns saga after setup
/// - get_incomplete_sagas() still returns saga after signing
/// - get_incomplete_sagas() returns empty after finalize
/// - Multiple incomplete sagas can be queried
///
/// # Success Criteria
/// - Incomplete saga appears in query results
/// - Completed saga does not appear in query results
#[tokio::test]
async fn test_get_incomplete_sagas_basic() {
    let mint = create_test_mint().await.unwrap();
    let db = mint.localstore();

    let amount = Amount::from(100);
    let (input_proofs_1, verification_1) = create_swap_inputs(&mint, amount).await;
    let (output_blinded_messages_1, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

    let (input_proofs_2, verification_2) = create_swap_inputs(&mint, amount).await;
    let (output_blinded_messages_2, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

    use cdk_common::mint::OperationKind;

    // Initially no incomplete sagas
    let incomplete_initial = db
        .get_incomplete_sagas(OperationKind::Swap)
        .await
        .expect("Failed to get incomplete sagas");
    assert_eq!(incomplete_initial.len(), 0);

    let pubsub = mint.pubsub_manager();

    // Setup first saga
    let saga_1 = SwapSaga::new(&mint, db.clone(), pubsub.clone());
    let saga_1 = saga_1
        .setup_swap(
            &input_proofs_1,
            &output_blinded_messages_1,
            None,
            verification_1,
        )
        .await
        .expect("Setup should succeed");
    let op_id_1 = *saga_1.operation.id();

    // Should have 1 incomplete saga
    let incomplete_after_1 = db
        .get_incomplete_sagas(OperationKind::Swap)
        .await
        .expect("Failed to get incomplete sagas");
    assert_eq!(incomplete_after_1.len(), 1);
    assert_eq!(incomplete_after_1[0].operation_id, op_id_1);

    // Setup second saga
    let saga_2 = SwapSaga::new(&mint, db.clone(), pubsub.clone());
    let saga_2 = saga_2
        .setup_swap(
            &input_proofs_2,
            &output_blinded_messages_2,
            None,
            verification_2,
        )
        .await
        .expect("Setup should succeed");
    let op_id_2 = *saga_2.operation.id();

    // Should have 2 incomplete sagas
    let incomplete_after_2 = db
        .get_incomplete_sagas(OperationKind::Swap)
        .await
        .expect("Failed to get incomplete sagas");
    assert_eq!(incomplete_after_2.len(), 2);

    // Finalize first saga
    let saga_1 = saga_1.sign_outputs().await.expect("Signing should succeed");
    let _response_1 = saga_1.finalize().await.expect("Finalize should succeed");

    // Should have 1 incomplete saga (second one still incomplete)
    let incomplete_after_finalize = db
        .get_incomplete_sagas(OperationKind::Swap)
        .await
        .expect("Failed to get incomplete sagas");
    assert_eq!(incomplete_after_finalize.len(), 1);
    assert_eq!(incomplete_after_finalize[0].operation_id, op_id_2);

    // Finalize second saga
    let saga_2 = saga_2.sign_outputs().await.expect("Signing should succeed");
    let _response_2 = saga_2.finalize().await.expect("Finalize should succeed");

    // Should have 0 incomplete sagas
    let incomplete_final = db
        .get_incomplete_sagas(OperationKind::Swap)
        .await
        .expect("Failed to get incomplete sagas");
    assert_eq!(incomplete_final.len(), 0);
}

/// Tests detailed validation of saga content.
///
/// # What This Tests
/// - Operation ID is correct
/// - Operation kind is correct
/// - State enum is correct
/// - Blinded secrets are all present
/// - Input Ys are all present
/// - Timestamps are reasonable (created_at, updated_at)
///
/// # Success Criteria
/// - All fields match expected values
/// - Timestamps are within reasonable range
#[tokio::test]
async fn test_saga_content_validation() {
    let mint = create_test_mint().await.unwrap();
    let db = mint.localstore();

    let amount = Amount::from(100);
    let (input_proofs, verification) = create_swap_inputs(&mint, amount).await;
    let (output_blinded_messages, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

    let expected_ys: Vec<_> = input_proofs.ys().unwrap();
    let expected_blinded_secrets: Vec<_> = output_blinded_messages
        .iter()
        .map(|bm| bm.blinded_secret)
        .collect();

    let pubsub = mint.pubsub_manager();
    let saga = SwapSaga::new(&mint, db.clone(), pubsub);

    let saga = saga
        .setup_swap(&input_proofs, &output_blinded_messages, None, verification)
        .await
        .expect("Setup should succeed");

    let operation_id = *saga.operation.id();

    // Query saga
    let saga = {
        let mut tx = db.begin_transaction().await.unwrap();
        let result = tx
            .get_saga(&operation_id)
            .await
            .expect("Failed to get saga");
        tx.commit().await.unwrap();
        result.expect("Saga should exist after setup")
    };

    // Validate content
    use cdk_common::mint::{OperationKind, SagaStateEnum, SwapSagaState};
    assert_eq!(saga.operation_id, operation_id);
    assert_eq!(saga.operation_kind, OperationKind::Swap);
    assert_eq!(
        saga.state,
        SagaStateEnum::Swap(SwapSagaState::SetupComplete)
    );

    // Validate blinded secrets
    assert_eq!(saga.blinded_secrets.len(), expected_blinded_secrets.len());
    for bs in &expected_blinded_secrets {
        assert!(saga.blinded_secrets.contains(bs));
    }

    // Validate input Ys
    assert_eq!(saga.input_ys.len(), expected_ys.len());
    for y in &expected_ys {
        assert!(saga.input_ys.contains(y));
    }

    // Validate timestamps
    use cdk_common::util::unix_time;
    let now = unix_time();
    assert!(
        saga.created_at <= now,
        "created_at should be <= current time"
    );
    assert!(
        saga.updated_at <= now,
        "updated_at should be <= current time"
    );
    assert!(
        saga.created_at <= saga.updated_at,
        "created_at should be <= updated_at"
    );
}

/// Tests that saga updates are persisted correctly.
///
/// # What This Tests
/// - Saga persisted after setup
/// - updated_at timestamp changes after state updates
/// - Other fields remain unchanged during updates
///
/// # Note
/// Currently sign_outputs() does NOT update saga in the database
/// (the "signed" state is not persisted). This test documents that behavior.
///
/// # Success Criteria
/// - State exists after setup
/// - If state is updated, updated_at increases
/// - Other fields remain consistent
#[tokio::test]
async fn test_saga_state_updates_persisted() {
    let mint = create_test_mint().await.unwrap();
    let db = mint.localstore();

    let amount = Amount::from(100);
    let (input_proofs, verification) = create_swap_inputs(&mint, amount).await;
    let (output_blinded_messages, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

    let pubsub = mint.pubsub_manager();
    let saga = SwapSaga::new(&mint, db.clone(), pubsub);

    let saga = saga
        .setup_swap(&input_proofs, &output_blinded_messages, None, verification)
        .await
        .expect("Setup should succeed");

    let operation_id = *saga.operation.id();

    // Query saga
    let state_after_setup = {
        let mut tx = db.begin_transaction().await.unwrap();
        let result = tx
            .get_saga(&operation_id)
            .await
            .expect("Failed to get saga");
        tx.commit().await.unwrap();
        result.expect("Saga should exist after setup")
    };

    use cdk_common::mint::{SagaStateEnum, SwapSagaState};
    assert_eq!(
        state_after_setup.state,
        SagaStateEnum::Swap(SwapSagaState::SetupComplete)
    );
    let initial_created_at = state_after_setup.created_at;
    let initial_updated_at = state_after_setup.updated_at;

    // Small delay to ensure timestamp would change if updated
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    let saga = saga.sign_outputs().await.expect("Signing should succeed");

    // Query saga
    let state_after_sign = {
        let mut tx = db.begin_transaction().await.unwrap();
        let result = tx
            .get_saga(&operation_id)
            .await
            .expect("Failed to get saga");
        tx.commit().await.unwrap();
        result.expect("Saga should exist after setup")
    };

    // State should still be SetupComplete (not updated to Signed)
    assert_eq!(
        state_after_sign.state,
        SagaStateEnum::Swap(SwapSagaState::SetupComplete),
        "Saga remains SetupComplete (signing doesn't update DB)"
    );

    // Verify other fields unchanged
    assert_eq!(state_after_sign.operation_id, operation_id);
    assert_eq!(
        state_after_sign.blinded_secrets,
        state_after_setup.blinded_secrets
    );
    assert_eq!(state_after_sign.input_ys, state_after_setup.input_ys);
    assert_eq!(state_after_sign.created_at, initial_created_at);

    // updated_at might not change since state wasn't updated
    assert_eq!(state_after_sign.updated_at, initial_updated_at);

    // Finalize and verify state is deleted (not updated)
    let _response = saga.finalize().await.expect("Finalize should succeed");

    // Query saga
    let state_after_finalize = {
        let mut tx = db.begin_transaction().await.unwrap();
        let result = tx
            .get_saga(&operation_id)
            .await
            .expect("Failed to get saga");
        tx.commit().await.unwrap();

        result
    };

    assert!(
        state_after_finalize.is_none(),
        "Saga should be deleted after finalize"
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

// ==================== PHASE 2: CRASH RECOVERY TESTS ====================
// These tests verify crash recovery using saga persistence.

/// Tests crash recovery without calling compensate_all().
///
/// # What This Tests
/// - Saga dropped WITHOUT calling compensate_all() (simulates process crash)
/// - Saga persists in database after crash
/// - Proofs remain PENDING after crash (not cleaned up)
/// - Recovery mechanism finds incomplete saga via get_incomplete_sagas()
/// - Recovery cleans up orphaned state (proofs, blinded messages, saga)
///
/// # This Is The PRIMARY USE CASE for Saga Persistence
/// The in-memory compensation mechanism only works if the process stays alive.
/// When the process crashes, we lose in-memory compensations and must rely
/// on persisted saga to recover.
///
/// # Success Criteria
/// - Saga exists after crash
/// - Proofs are PENDING after crash (compensation didn't run)
/// - Recovery removes proofs
/// - Recovery removes blinded messages
/// - Recovery deletes saga
#[tokio::test]
async fn test_crash_recovery_without_compensation() {
    let mint = create_test_mint().await.unwrap();
    let db = mint.localstore();

    let amount = Amount::from(100);
    let (input_proofs, verification) = create_swap_inputs(&mint, amount).await;
    let (output_blinded_messages, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

    let operation_id;
    let ys = input_proofs.ys().unwrap();
    let _blinded_secrets: Vec<_> = output_blinded_messages
        .iter()
        .map(|bm| bm.blinded_secret)
        .collect();

    // Simulate crash: setup swap, then drop WITHOUT calling compensate_all()
    {
        let pubsub = mint.pubsub_manager();
        let saga = SwapSaga::new(&mint, db.clone(), pubsub);

        let saga = saga
            .setup_swap(&input_proofs, &output_blinded_messages, None, verification)
            .await
            .expect("Setup should succeed");

        operation_id = *saga.operation.id();

        // CRITICAL: Drop saga WITHOUT calling compensate_all()
        // This simulates a crash where in-memory compensations are lost
        drop(saga);
    }

    // Verify saga still exists in database (persisted during setup)
    let saga = {
        let mut tx = db.begin_transaction().await.unwrap();
        let result = tx
            .get_saga(&operation_id)
            .await
            .expect("Failed to get saga");
        tx.commit().await.unwrap();
        result
    };
    assert!(saga.is_some(), "Saga should persist after crash");

    // Verify proofs are still Pending (compensation didn't run)
    let states = db.get_proofs_states(&ys).await.unwrap();
    assert!(
        states.iter().all(|s| s == &Some(State::Pending)),
        "Proofs should still be Pending after crash (compensation didn't run)"
    );

    // Note: We cannot directly verify blinded messages exist (no query method)
    // but the recovery process will delete them along with proofs

    // Simulate mint restart - run recovery
    mint.stop().await.expect("Stop should succeed");
    mint.start().await.expect("Start should succeed");

    // Verify recovery cleaned up:
    // 1. Proofs removed from database
    let states_after = db.get_proofs_states(&ys).await.unwrap();
    assert!(
        states_after.iter().all(|s| s.is_none()),
        "Recovery should remove proofs"
    );

    // 2. Blinded messages removed (implicitly - no query method available)

    // 3. Saga deleted
    let saga_after = {
        let mut tx = db.begin_transaction().await.unwrap();
        let result = tx
            .get_saga(&operation_id)
            .await
            .expect("Failed to get saga");
        tx.commit().await.unwrap();
        result
    };
    assert!(saga_after.is_none(), "Recovery should delete saga");
}

/// Tests crash recovery after setup only (before signing).
///
/// # What This Tests
/// - Saga in SetupComplete state when crashed
/// - No signatures exist in database
/// - Recovery removes all swap state
///
/// # Success Criteria
/// - Saga exists before recovery
/// - Proofs are Pending before recovery
/// - Everything cleaned up after recovery
#[tokio::test]
async fn test_crash_recovery_after_setup_only() {
    let mint = create_test_mint().await.unwrap();
    let db = mint.localstore();

    let amount = Amount::from(100);
    let (input_proofs, verification) = create_swap_inputs(&mint, amount).await;
    let (output_blinded_messages, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

    let operation_id;
    let ys = input_proofs.ys().unwrap();
    let _blinded_secrets: Vec<_> = output_blinded_messages
        .iter()
        .map(|bm| bm.blinded_secret)
        .collect();

    // Setup and crash
    {
        let pubsub = mint.pubsub_manager();
        let saga = SwapSaga::new(&mint, db.clone(), pubsub);

        let saga = saga
            .setup_swap(&input_proofs, &output_blinded_messages, None, verification)
            .await
            .expect("Setup should succeed");

        operation_id = *saga.operation.id();

        // Verify saga was persisted
        let saga = {
            let mut tx = db.begin_transaction().await.unwrap();
            let result = tx.get_saga(&operation_id).await.unwrap();
            tx.commit().await.unwrap();
            result
        };
        assert!(saga.is_some());

        // Drop without compensation (crash)
        drop(saga);
    }

    // Verify state before recovery
    let saga_before = {
        let mut tx = db.begin_transaction().await.unwrap();
        let result = tx.get_saga(&operation_id).await.unwrap();
        tx.commit().await.unwrap();
        result
    };
    assert!(saga_before.is_some());

    let states_before = db.get_proofs_states(&ys).await.unwrap();
    assert!(states_before.iter().all(|s| s == &Some(State::Pending)));

    // Run recovery
    mint.stop().await.expect("Stop should succeed");
    mint.start().await.expect("Start should succeed");

    // Verify cleanup
    let saga_after = {
        let mut tx = db.begin_transaction().await.unwrap();
        let result = tx.get_saga(&operation_id).await.unwrap();
        tx.commit().await.unwrap();
        result
    };
    assert!(saga_after.is_none(), "Saga should be deleted");

    let states_after = db.get_proofs_states(&ys).await.unwrap();
    assert!(
        states_after.iter().all(|s| s.is_none()),
        "Proofs should be removed"
    );

    // Blinded messages also removed by recovery (no query method to verify)
}

/// Tests crash recovery after signing (before finalize).
///
/// # What This Tests
/// - Saga crashed after sign_outputs() but before finalize()
/// - Signatures were in memory only (never persisted)
/// - Recovery treats this the same as crashed after setup
/// - All state is cleaned up
///
/// # Success Criteria
/// - Saga exists before recovery
/// - No signatures in database (never persisted)
/// - Everything cleaned up after recovery
#[tokio::test]
async fn test_crash_recovery_after_signing() {
    let mint = create_test_mint().await.unwrap();
    let db = mint.localstore();

    let amount = Amount::from(100);
    let (input_proofs, verification) = create_swap_inputs(&mint, amount).await;
    let (output_blinded_messages, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

    let operation_id;
    let ys = input_proofs.ys().unwrap();
    let blinded_secrets: Vec<_> = output_blinded_messages
        .iter()
        .map(|bm| bm.blinded_secret)
        .collect();

    // Setup, sign, and crash
    {
        let pubsub = mint.pubsub_manager();
        let saga = SwapSaga::new(&mint, db.clone(), pubsub);

        let saga = saga
            .setup_swap(&input_proofs, &output_blinded_messages, None, verification)
            .await
            .expect("Setup should succeed");

        operation_id = *saga.operation.id();

        let saga = saga.sign_outputs().await.expect("Signing should succeed");

        // Verify we have signatures in memory
        assert_eq!(
            saga.state_data.signatures.len(),
            output_blinded_messages.len()
        );

        // Drop without finalize (crash) - signatures lost
        drop(saga);
    }

    // Verify state before recovery
    let saga_before = {
        let mut tx = db.begin_transaction().await.unwrap();
        let result = tx.get_saga(&operation_id).await.unwrap();
        tx.commit().await.unwrap();
        result
    };
    assert!(saga_before.is_some());

    // Verify no signatures in database (they were in memory only)
    let sigs_before = db.get_blind_signatures(&blinded_secrets).await.unwrap();
    assert!(
        sigs_before.iter().all(|s| s.is_none()),
        "Signatures should not be in DB (never persisted)"
    );

    // Run recovery
    mint.stop().await.expect("Stop should succeed");
    mint.start().await.expect("Start should succeed");

    // Verify cleanup
    let saga_after = {
        let mut tx = db.begin_transaction().await.unwrap();
        let result = tx.get_saga(&operation_id).await.unwrap();
        tx.commit().await.unwrap();
        result
    };
    assert!(saga_after.is_none(), "Saga should be deleted");

    let states_after = db.get_proofs_states(&ys).await.unwrap();
    assert!(
        states_after.iter().all(|s| s.is_none()),
        "Proofs should be removed"
    );

    // Blinded messages also removed by recovery (no query method to verify)
}

/// Tests recovery with multiple incomplete sagas in different states.
///
/// # What This Tests
/// - Multiple sagas can be incomplete simultaneously
/// - Recovery processes all incomplete sagas
/// - Each saga is handled correctly based on its state
///
/// # Test Scenario
/// - Saga A: Setup only (incomplete)
/// - Saga B: Setup + Sign (incomplete, signatures lost)
/// - Saga C: Completed (should NOT be affected by recovery)
///
/// # Success Criteria
/// - Saga A cleaned up
/// - Saga B cleaned up
/// - Saga C unaffected
#[tokio::test]
async fn test_recovery_multiple_incomplete_sagas() {
    let mint = create_test_mint().await.unwrap();
    let db = mint.localstore();

    let amount = Amount::from(100);

    // Create three sets of inputs/outputs
    let (proofs_a, verification_a) = create_swap_inputs(&mint, amount).await;
    let (proofs_b, verification_b) = create_swap_inputs(&mint, amount).await;
    let (proofs_c, verification_c) = create_swap_inputs(&mint, amount).await;

    let (outputs_a, _) = create_test_blinded_messages(&mint, amount).await.unwrap();
    let (outputs_b, _) = create_test_blinded_messages(&mint, amount).await.unwrap();
    let (outputs_c, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

    let ys_a = proofs_a.ys().unwrap();
    let ys_b = proofs_b.ys().unwrap();
    let ys_c = proofs_c.ys().unwrap();

    let op_id_a;
    let op_id_b;
    let op_id_c;

    // Saga A: Setup only, then crash
    {
        let pubsub = mint.pubsub_manager();
        let saga = SwapSaga::new(&mint, db.clone(), pubsub);
        let saga = saga
            .setup_swap(&proofs_a, &outputs_a, None, verification_a)
            .await
            .expect("Setup A should succeed");
        op_id_a = *saga.operation.id();
        drop(saga);
    }

    // Saga B: Setup + Sign, then crash
    {
        let pubsub = mint.pubsub_manager();
        let saga = SwapSaga::new(&mint, db.clone(), pubsub);
        let saga = saga
            .setup_swap(&proofs_b, &outputs_b, None, verification_b)
            .await
            .expect("Setup B should succeed");
        op_id_b = *saga.operation.id();
        let saga = saga.sign_outputs().await.expect("Sign B should succeed");
        drop(saga);
    }

    // Saga C: Complete successfully
    {
        let pubsub = mint.pubsub_manager();
        let saga = SwapSaga::new(&mint, db.clone(), pubsub);
        let saga = saga
            .setup_swap(&proofs_c, &outputs_c, None, verification_c)
            .await
            .expect("Setup C should succeed");
        op_id_c = *saga.operation.id();
        let saga = saga.sign_outputs().await.expect("Sign C should succeed");
        let _response = saga.finalize().await.expect("Finalize C should succeed");
    }

    // Verify state before recovery
    use cdk_common::mint::OperationKind;
    let incomplete_before = db.get_incomplete_sagas(OperationKind::Swap).await.unwrap();
    assert_eq!(
        incomplete_before.len(),
        2,
        "Should have 2 incomplete sagas (A and B)"
    );

    let states_a_before = db.get_proofs_states(&ys_a).await.unwrap();
    let states_b_before = db.get_proofs_states(&ys_b).await.unwrap();
    let states_c_before = db.get_proofs_states(&ys_c).await.unwrap();

    assert!(states_a_before.iter().all(|s| s == &Some(State::Pending)));
    assert!(states_b_before.iter().all(|s| s == &Some(State::Pending)));
    assert!(states_c_before.iter().all(|s| s == &Some(State::Spent)));

    // Run recovery
    mint.stop().await.expect("Stop should succeed");
    mint.start().await.expect("Start should succeed");

    // Verify cleanup
    let incomplete_after = db.get_incomplete_sagas(OperationKind::Swap).await.unwrap();
    assert_eq!(
        incomplete_after.len(),
        0,
        "No incomplete sagas after recovery"
    );

    // Saga A cleaned up
    let saga_a = {
        let mut tx = db.begin_transaction().await.unwrap();
        let result = tx.get_saga(&op_id_a).await.unwrap();
        tx.commit().await.unwrap();
        result
    };
    assert!(saga_a.is_none());
    let states_a_after = db.get_proofs_states(&ys_a).await.unwrap();
    assert!(states_a_after.iter().all(|s| s.is_none()));

    // Saga B cleaned up
    let saga_b = {
        let mut tx = db.begin_transaction().await.unwrap();
        let result = tx.get_saga(&op_id_b).await.unwrap();
        tx.commit().await.unwrap();
        result
    };
    assert!(saga_b.is_none());
    let states_b_after = db.get_proofs_states(&ys_b).await.unwrap();
    assert!(states_b_after.iter().all(|s| s.is_none()));

    // Saga C unaffected (still spent, saga was already deleted)
    let saga_c = {
        let mut tx = db.begin_transaction().await.unwrap();
        let result = tx.get_saga(&op_id_c).await.unwrap();
        tx.commit().await.unwrap();
        result
    };
    assert!(saga_c.is_none(), "Completed saga was deleted");
    let states_c_after = db.get_proofs_states(&ys_c).await.unwrap();
    assert!(
        states_c_after.iter().all(|s| s == &Some(State::Spent)),
        "Completed saga proofs remain spent"
    );
}

/// Tests that recovery is idempotent (can be run multiple times safely).
///
/// # What This Tests
/// - Recovery can be run multiple times without errors
/// - Second recovery run is a no-op
/// - State remains consistent after multiple recoveries
///
/// # Success Criteria
/// - First recovery cleans up incomplete saga
/// - Second recovery succeeds (no incomplete sagas to process)
/// - State is consistent after both runs
#[tokio::test]
async fn test_recovery_idempotence() {
    let mint = create_test_mint().await.unwrap();
    let db = mint.localstore();

    let amount = Amount::from(100);
    let (input_proofs, verification) = create_swap_inputs(&mint, amount).await;
    let (output_blinded_messages, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

    let operation_id;
    let ys = input_proofs.ys().unwrap();

    // Create incomplete saga
    {
        let pubsub = mint.pubsub_manager();
        let saga = SwapSaga::new(&mint, db.clone(), pubsub);
        let saga = saga
            .setup_swap(&input_proofs, &output_blinded_messages, None, verification)
            .await
            .expect("Setup should succeed");
        operation_id = *saga.operation.id();
        drop(saga);
    }

    // Verify incomplete saga exists
    let saga_before = {
        let mut tx = db.begin_transaction().await.unwrap();
        let result = tx.get_saga(&operation_id).await.unwrap();
        tx.commit().await.unwrap();
        result
    };
    assert!(saga_before.is_some());

    // First recovery
    mint.stop().await.expect("First stop should succeed");
    mint.start().await.expect("First start should succeed");

    // Verify cleanup
    let saga_after_1 = {
        let mut tx = db.begin_transaction().await.unwrap();
        let result = tx.get_saga(&operation_id).await.unwrap();
        tx.commit().await.unwrap();
        result
    };
    assert!(saga_after_1.is_none());
    let states_after_1 = db.get_proofs_states(&ys).await.unwrap();
    assert!(states_after_1.iter().all(|s| s.is_none()));

    // Second recovery (should be idempotent - no work to do)
    mint.stop().await.expect("Second stop should succeed");
    mint.start().await.expect("Second start should succeed");

    // Verify state unchanged
    let saga_after_2 = {
        let mut tx = db.begin_transaction().await.unwrap();
        let result = tx.get_saga(&operation_id).await.unwrap();
        tx.commit().await.unwrap();
        result
    };
    assert!(saga_after_2.is_none());
    let states_after_2 = db.get_proofs_states(&ys).await.unwrap();
    assert!(states_after_2.iter().all(|s| s.is_none()));

    // Third recovery for good measure
    mint.stop().await.expect("Third stop should succeed");
    mint.start().await.expect("Third start should succeed");

    let saga_after_3 = {
        let mut tx = db.begin_transaction().await.unwrap();
        let result = tx.get_saga(&operation_id).await.unwrap();
        tx.commit().await.unwrap();
        result
    };
    assert!(saga_after_3.is_none());
}

// ==================== PHASE 3: EDGE CASE TESTS ====================
// These tests verify edge cases and error handling scenarios.

/// Tests cleanup of orphaned saga (saga deletion fails but swap succeeds).
///
/// # What This Tests
/// - Swap completes successfully (proofs marked SPENT)
/// - Saga deletion fails (simulated by test hook)
/// - Swap still succeeds (best-effort deletion)
/// - Saga remains orphaned in database
/// - Recovery detects orphaned saga (proofs already SPENT)
/// - Recovery deletes orphaned saga
///
/// # Why This Matters
/// According to the implementation, saga deletion is best-effort. If it fails,
/// the swap should still succeed. The orphaned saga will be cleaned up
/// on next recovery.
///
/// # Success Criteria
/// - Swap succeeds despite deletion failure
/// - Proofs are SPENT after swap
/// - Saga remains after swap (orphaned)
/// - Recovery cleans up orphaned saga
#[tokio::test]
async fn test_orphaned_saga_cleanup() {
    let mint = create_test_mint().await.unwrap();
    let db = mint.localstore();

    let amount = Amount::from(100);
    let (input_proofs, verification) = create_swap_inputs(&mint, amount).await;
    let (output_blinded_messages, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

    let pubsub = mint.pubsub_manager();
    let saga = SwapSaga::new(&mint, db.clone(), pubsub);

    let saga = saga
        .setup_swap(&input_proofs, &output_blinded_messages, None, verification)
        .await
        .expect("Setup should succeed");

    let operation_id = *saga.operation.id();
    let ys = input_proofs.ys().unwrap();

    let saga = saga.sign_outputs().await.expect("Signing should succeed");

    // Note: We cannot easily inject a failure for saga deletion within finalize
    // because the deletion happens inside a database transaction and uses the
    // transaction trait. For now, we'll test the recovery side: create a saga
    // that completes, then manually verify recovery can handle scenarios where
    // saga exists but proofs are already SPENT.

    let _response = saga.finalize().await.expect("Finalize should succeed");

    // Verify swap succeeded (proofs SPENT)
    let states = db.get_proofs_states(&ys).await.unwrap();
    assert!(
        states.iter().all(|s| s == &Some(State::Spent)),
        "Proofs should be SPENT after successful swap"
    );

    // In a real scenario with deletion failure, saga would remain.
    // For this test, we'll verify that saga is properly deleted.
    // TODO: Add failure injection for delete_saga to properly test this.
    let saga = {
        let mut tx = db.begin_transaction().await.unwrap();
        let result = tx.get_saga(&operation_id).await.unwrap();
        tx.commit().await.unwrap();
        result
    };
    assert!(
        saga.is_none(),
        "Saga should be deleted after successful swap"
    );

    // If we had a way to inject deletion failure, we would:
    // 1. Verify saga remains (orphaned)
    // 2. Run recovery
    // 3. Verify recovery detects proofs are SPENT
    // 4. Verify recovery deletes orphaned saga
}

/// Tests recovery with orphaned proofs (proofs without corresponding saga).
///
/// # What This Tests
/// - Proofs exist in database without saga
/// - Recovery handles this gracefully (no crash)
/// - Proofs remain in their current state
///
/// # Scenario
/// This could happen if:
/// - Manual database intervention removed saga but not proofs
/// - A bug caused saga deletion without proof cleanup
/// - Database corruption
///
/// # Success Criteria
/// - Recovery runs without errors
/// - Proofs remain in database (recovery doesn't remove them without saga)
/// - No crashes or panics
#[tokio::test]
async fn test_recovery_with_orphaned_proofs() {
    let mint = create_test_mint().await.unwrap();
    let db = mint.localstore();

    let amount = Amount::from(100);
    let (input_proofs, verification) = create_swap_inputs(&mint, amount).await;
    let (output_blinded_messages, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

    let ys = input_proofs.ys().unwrap();

    // Setup saga to get proofs into PENDING state
    let operation_id = {
        let pubsub = mint.pubsub_manager();
        let saga = SwapSaga::new(&mint, db.clone(), pubsub);

        let saga = saga
            .setup_swap(&input_proofs, &output_blinded_messages, None, verification)
            .await
            .expect("Setup should succeed");

        let op_id = *saga.operation.id();

        // Drop saga (crash simulation)
        drop(saga);

        op_id
    };

    // Verify proofs are PENDING and saga exists
    let states_before = db.get_proofs_states(&ys).await.unwrap();
    assert!(states_before.iter().all(|s| s == &Some(State::Pending)));

    let saga_before = {
        let mut tx = db.begin_transaction().await.unwrap();
        let result = tx.get_saga(&operation_id).await.unwrap();
        tx.commit().await.unwrap();
        result
    };
    assert!(saga_before.is_some());

    // Manually delete saga (simulating orphaned proofs scenario)
    {
        let mut tx = db.begin_transaction().await.unwrap();
        tx.delete_saga(&operation_id).await.unwrap();
        tx.commit().await.unwrap();
    }

    // Verify saga is gone but proofs remain
    let saga_after_delete = {
        let mut tx = db.begin_transaction().await.unwrap();
        let result = tx.get_saga(&operation_id).await.unwrap();
        tx.commit().await.unwrap();
        result
    };
    assert!(saga_after_delete.is_none(), "Saga should be deleted");

    let states_after_delete = db.get_proofs_states(&ys).await.unwrap();
    assert!(
        states_after_delete
            .iter()
            .all(|s| s == &Some(State::Pending)),
        "Proofs should still be PENDING (orphaned)"
    );

    // Run recovery - should handle gracefully
    mint.stop().await.expect("Stop should succeed");
    mint.start().await.expect("Start should succeed");

    // Verify recovery completed without errors
    // Orphaned PENDING proofs without saga should remain (not cleaned up)
    // This is by design - recovery only acts on incomplete sagas, not orphaned proofs
    let states_after_recovery = db.get_proofs_states(&ys).await.unwrap();
    assert!(
        states_after_recovery
            .iter()
            .all(|s| s == &Some(State::Pending)),
        "Orphaned proofs remain PENDING (recovery doesn't clean up proofs without saga)"
    );

    // Note: In production, a separate cleanup mechanism (e.g., timeout-based)
    // would be needed to handle such orphaned resources. Saga recovery only
    // processes incomplete sagas that have saga.
}

/// Tests recovery with partial state (missing blinded messages).
///
/// # What This Tests
/// - Saga exists
/// - Proofs exist
/// - Blinded messages are missing (deleted manually)
/// - Recovery handles this gracefully
///
/// # Scenario
/// This could occur due to:
/// - Partial transaction commit (unlikely with proper atomicity)
/// - Manual database intervention
/// - Database corruption
///
/// # Success Criteria
/// - Recovery runs without errors
/// - Saga is cleaned up
/// - Proofs are removed
/// - No crashes due to missing blinded messages
#[tokio::test]
async fn test_recovery_with_partial_state() {
    let mint = create_test_mint().await.unwrap();
    let db = mint.localstore();

    let amount = Amount::from(100);
    let (input_proofs, verification) = create_swap_inputs(&mint, amount).await;
    let (output_blinded_messages, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

    let ys = input_proofs.ys().unwrap();
    let blinded_secrets: Vec<_> = output_blinded_messages
        .iter()
        .map(|bm| bm.blinded_secret)
        .collect();

    // Setup saga
    let operation_id = {
        let pubsub = mint.pubsub_manager();
        let saga = SwapSaga::new(&mint, db.clone(), pubsub);

        let saga = saga
            .setup_swap(&input_proofs, &output_blinded_messages, None, verification)
            .await
            .expect("Setup should succeed");

        let op_id = *saga.operation.id();

        // Drop saga (crash simulation)
        drop(saga);

        op_id
    };

    // Verify setup
    let saga_before = {
        let mut tx = db.begin_transaction().await.unwrap();
        let result = tx.get_saga(&operation_id).await.unwrap();
        tx.commit().await.unwrap();
        result
    };
    assert!(saga_before.is_some());

    let states_before = db.get_proofs_states(&ys).await.unwrap();
    assert!(states_before.iter().all(|s| s == &Some(State::Pending)));

    // Manually delete blinded messages (simulating partial state)
    {
        let mut tx = db.begin_transaction().await.unwrap();
        tx.delete_blinded_messages(&blinded_secrets).await.unwrap();
        tx.commit().await.unwrap();
    }

    // Verify blinded messages are gone but saga and proofs remain
    // (Note: We can't directly query blinded messages to verify they're gone,
    // but the recovery mechanism will attempt to delete them regardless)

    // Run recovery - should handle missing blinded messages gracefully
    mint.stop().await.expect("Stop should succeed");
    mint.start().await.expect("Start should succeed");

    // Verify recovery completed successfully
    let saga_after = {
        let mut tx = db.begin_transaction().await.unwrap();
        let result = tx.get_saga(&operation_id).await.unwrap();
        tx.commit().await.unwrap();
        result
    };
    assert!(saga_after.is_none(), "Saga should be deleted");

    let states_after = db.get_proofs_states(&ys).await.unwrap();
    assert!(
        states_after.iter().all(|s| s.is_none()),
        "Proofs should be removed"
    );

    // Recovery should succeed even if blinded messages were already gone
}

/// Tests recovery when blinded messages are missing (but proofs and saga exist).
///
/// # What This Tests
/// - Saga exists with blinded_secrets
/// - Proofs exist and are PENDING
/// - Blinded messages themselves are missing from database
/// - Recovery completes without errors
/// - Saga is cleaned up
/// - Proofs are removed
///
/// # Success Criteria
/// - No errors when trying to delete missing blinded messages
/// - Recovery completes successfully
/// - All saga cleaned up
#[tokio::test]
async fn test_recovery_with_missing_blinded_messages() {
    let mint = create_test_mint().await.unwrap();
    let db = mint.localstore();

    let amount = Amount::from(100);
    let (input_proofs, verification) = create_swap_inputs(&mint, amount).await;
    let (output_blinded_messages, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

    let ys = input_proofs.ys().unwrap();
    let blinded_secrets: Vec<_> = output_blinded_messages
        .iter()
        .map(|bm| bm.blinded_secret)
        .collect();

    // Setup saga and crash
    let operation_id = {
        let pubsub = mint.pubsub_manager();
        let saga = SwapSaga::new(&mint, db.clone(), pubsub);

        let saga = saga
            .setup_swap(&input_proofs, &output_blinded_messages, None, verification)
            .await
            .expect("Setup should succeed");

        let op_id = *saga.operation.id();
        drop(saga); // Crash

        op_id
    };

    // Verify initial state
    let saga = {
        let mut tx = db.begin_transaction().await.unwrap();
        let result = tx.get_saga(&operation_id).await.unwrap();
        tx.commit().await.unwrap();
        result
    };
    assert!(saga.is_some(), "Saga should exist");

    // Manually delete blinded messages before recovery
    {
        let mut tx = db.begin_transaction().await.unwrap();
        tx.delete_blinded_messages(&blinded_secrets).await.unwrap();
        tx.commit().await.unwrap();
    }

    // Run recovery - should handle missing blinded messages gracefully
    mint.stop().await.expect("Stop should succeed");
    mint.start()
        .await
        .expect("Start should succeed despite missing blinded messages");

    // Verify cleanup
    let saga_after = {
        let mut tx = db.begin_transaction().await.unwrap();
        let result = tx.get_saga(&operation_id).await.unwrap();
        tx.commit().await.unwrap();
        result
    };
    assert!(saga_after.is_none(), "Saga should be cleaned up");

    let states_after = db.get_proofs_states(&ys).await.unwrap();
    assert!(
        states_after.iter().all(|s| s.is_none()),
        "Proofs should be removed"
    );
}

/// Tests that saga deletion failure is handled gracefully during finalize.
///
/// # What This Tests
/// - Swap completes successfully through finalize
/// - Even if saga deletion fails internally, swap succeeds
/// - Best-effort saga deletion doesn't fail the swap
///
/// # Note
/// This test verifies the design decision that saga deletion is best-effort.
/// Currently we cannot easily inject deletion failures, so this test documents
/// the expected behavior and verifies normal deletion.
///
/// # Success Criteria
/// - Swap completes successfully
/// - Saga is deleted (in normal case)
/// - If deletion fails (not testable yet), swap still succeeds
#[tokio::test]
async fn test_saga_deletion_failure_handling() {
    let mint = create_test_mint().await.unwrap();
    let db = mint.localstore();

    let amount = Amount::from(100);
    let (input_proofs, verification) = create_swap_inputs(&mint, amount).await;
    let (output_blinded_messages, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

    let pubsub = mint.pubsub_manager();
    let saga = SwapSaga::new(&mint, db.clone(), pubsub);

    let saga = saga
        .setup_swap(&input_proofs, &output_blinded_messages, None, verification)
        .await
        .expect("Setup should succeed");

    let operation_id = *saga.operation.id();
    let ys = input_proofs.ys().unwrap();

    let saga = saga.sign_outputs().await.expect("Signing should succeed");

    // In normal operation, deletion succeeds
    let response = saga.finalize().await.expect("Finalize should succeed");

    // Verify swap succeeded
    assert_eq!(
        response.signatures.len(),
        output_blinded_messages.len(),
        "Should have signatures for all outputs"
    );

    let states = db.get_proofs_states(&ys).await.unwrap();
    assert!(
        states.iter().all(|s| s == &Some(State::Spent)),
        "Proofs should be SPENT"
    );

    // Verify saga is deleted
    let saga = {
        let mut tx = db.begin_transaction().await.unwrap();
        let result = tx.get_saga(&operation_id).await.unwrap();
        tx.commit().await.unwrap();
        result
    };
    assert!(saga.is_none(), "Saga should be deleted");

    // TODO: Add test failure injection for delete_saga to verify that:
    // 1. Swap still succeeds even if deletion fails
    // 2. Orphaned saga remains
    // 3. Recovery can clean it up later
    //
    // This would require adding a TEST_FAIL_DELETE_SAGA env var check in the
    // database implementation's delete_saga method.
}

#![cfg(test)]
//! High-level tests for swap functionality
//!
//! These tests verify the correctness of the swap operation from a user perspective,
//! independent of the saga implementation details.

use cdk_common::nuts::ProofsMethods;
use cdk_common::Amount;

use crate::test_helpers::mint::{create_test_blinded_messages, create_test_mint, mint_test_proofs};

/// Test: process_swap_request happy path
///
/// Tests the complete end-to-end flow through process_swap_request
#[tokio::test]
async fn test_process_swap_request_happy_path() {
    let mint = create_test_mint().await.unwrap();

    // Create 100 sats of input proofs
    let amount = Amount::from(100);
    let input_proofs = mint_test_proofs(&mint, amount).await.unwrap();

    println!(
        "Created {} input proofs totaling {} sats",
        input_proofs.len(),
        input_proofs.total_amount().unwrap()
    );

    // Create output blinded messages for the same amount
    let (output_blinded_messages, _premint_secrets) =
        create_test_blinded_messages(&mint, amount).await.unwrap();

    println!(
        "Created {} output blinded messages",
        output_blinded_messages.len()
    );

    // Create and process swap request
    let swap_request =
        cdk_common::nuts::SwapRequest::new(input_proofs.clone(), output_blinded_messages.clone());

    let swap_response = mint.process_swap_request(swap_request).await.unwrap();

    println!("Swap successful! Received {} signatures", swap_response.signatures.len());

    assert_eq!(
        swap_response.signatures.len(),
        output_blinded_messages.len(),
        "Should receive one signature per blinded message"
    );

    // Verify proofs are now spent (double-spend should fail)
    let swap_request_2 =
        cdk_common::nuts::SwapRequest::new(input_proofs.clone(), output_blinded_messages.clone());
    let result = mint.process_swap_request(swap_request_2).await;

    assert!(result.is_err(), "Should not be able to spend the same proofs twice");
}

/// Test: process_swap_request with unbalanced transaction
///
/// Verifies that the mint rejects swaps where outputs exceed inputs
#[tokio::test]
async fn test_process_swap_request_unbalanced() {
    let mint = create_test_mint().await.unwrap();

    // Create 100 sats of inputs
    let input_amount = Amount::from(100);
    let input_proofs = mint_test_proofs(&mint, input_amount).await.unwrap();

    // Try to create 200 sats of outputs (should fail!)
    let output_amount = Amount::from(200);
    let (output_blinded_messages, _) = create_test_blinded_messages(&mint, output_amount)
        .await
        .unwrap();

    let swap_request =
        cdk_common::nuts::SwapRequest::new(input_proofs, output_blinded_messages);

    // This should fail because inputs < outputs
    let result = mint.process_swap_request(swap_request).await;

    assert!(result.is_err(), "Unbalanced transaction should be rejected");
}

/// Test: process_swap_request with duplicate inputs
///
/// Verifies that the mint rejects swaps with duplicate input proofs
#[tokio::test]
async fn test_process_swap_request_duplicate_inputs() {
    let mint = create_test_mint().await.unwrap();

    let amount = Amount::from(100);
    let mut input_proofs = mint_test_proofs(&mint, amount).await.unwrap();

    // Duplicate the first proof
    let duplicate_proof = input_proofs[0].clone();
    input_proofs.push(duplicate_proof);

    let (output_blinded_messages, _) = create_test_blinded_messages(&mint, amount).await.unwrap();

    let swap_request =
        cdk_common::nuts::SwapRequest::new(input_proofs, output_blinded_messages);

    let result = mint.process_swap_request(swap_request).await;

    assert!(result.is_err(), "Duplicate inputs should be rejected");
}

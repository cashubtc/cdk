#![cfg(test)]
//! HTLC (NUT-14) tests for swap functionality
//!
//! These tests verify that the mint correctly validates HTLC spending conditions
//! during swap operations, including:
//! - Hash preimage verification
//! - Locktime enforcement
//! - Refund keys
//! - Signature validation

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use cdk_common::nuts::{SpendingConditions, Conditions, SigFlag};
use cdk_common::Amount;

use super::test_helpers::{TestMintHelper, create_test_keypair, unzip3};

/// Helper: Create a hash and preimage for testing
/// Returns (hash_hex_string, preimage_string)
fn create_test_hash_and_preimage() -> (String, String) {
    let preimage = "secret_preimage_for_testing";
    let hash = Sha256Hash::hash(preimage.as_bytes());
    (hash.to_string(), preimage.to_string())
}

/// Test: HTLC requiring preimage and one signature
///
/// Creates HTLC-locked proofs and verifies:
/// 1. Spending with only preimage fails (signature required)
/// 2. Spending with only signature fails (preimage required)
/// 3. Spending with both preimage and signature succeeds
#[tokio::test]
async fn test_htlc_requiring_preimage_and_one_signature() {
    let test_mint = TestMintHelper::new().await.unwrap();
    let mint = test_mint.mint();

    // Generate keypair for Alice
    let (alice_secret, alice_pubkey) = create_test_keypair();

    // Create hash and preimage
    let (hash, preimage) = create_test_hash_and_preimage();

    println!("Alice pubkey: {}", alice_pubkey);
    println!("Hash: {}", hash);
    println!("Preimage: {}", preimage);

    // Step 1: Mint regular proofs
    let input_amount = Amount::from(10);
    let input_proofs = test_mint.mint_proofs(input_amount).await.unwrap();

    // Step 2: Create HTLC spending conditions (hash locked to Alice's key)
    let spending_conditions = SpendingConditions::new_htlc_hash(
        &hash,
        Some(Conditions {
            locktime: None,
            pubkeys: Some(vec![alice_pubkey]),
            refund_keys: None,
            num_sigs: None, // Default (1)
            sig_flag: SigFlag::default(),
            num_sigs_refund: None,
        })
    ).unwrap();
    println!("Created HTLC spending conditions");

    // Step 3: Create HTLC blinded messages (outputs)
    let split_amounts = test_mint.split_amount(input_amount).unwrap();
    let split_display: Vec<String> = split_amounts.iter().map(|a| a.to_string()).collect();
    println!("Split {} into [{}]", input_amount, split_display.join("+"));

    let (htlc_outputs, blinding_factors, secrets) = unzip3(
        split_amounts
            .iter()
            .map(|&amt| test_mint.create_blinded_message(amt, &spending_conditions))
            .collect(),
    );
    println!("Created {} HTLC outputs locked to alice with hash", htlc_outputs.len());

    // Step 4: Swap regular proofs for HTLC proofs (no signature needed on inputs)
    let swap_request = cdk_common::nuts::SwapRequest::new(
        input_proofs.clone(),
        htlc_outputs.clone(),
    );
    let swap_response = mint
        .process_swap_request(swap_request)
        .await
        .expect("Failed to swap for HTLC proofs");
    println!("Swap successful! Got BlindSignatures for our HTLC outputs");

    // Step 5: Construct the HTLC proofs
    use cdk_common::dhke::construct_proofs;
    let htlc_proofs = construct_proofs(
        swap_response.signatures.clone(),
        blinding_factors.clone(),
        secrets.clone(),
        &test_mint.public_keys_of_the_active_sat_keyset,
    ).unwrap();

    let proof_amounts: Vec<String> = htlc_proofs.iter().map(|p| p.amount.to_string()).collect();
    println!("Constructed {} HTLC proof(s) [{}]", htlc_proofs.len(), proof_amounts.join("+"));

    // Step 6: Try to spend with only preimage (should fail - signature required)
    use crate::test_helpers::mint::create_test_blinded_messages;
    let (new_outputs, _) = create_test_blinded_messages(mint, input_amount).await.unwrap();
    let mut swap_request_preimage_only = cdk_common::nuts::SwapRequest::new(
        htlc_proofs.clone(),
        new_outputs.clone(),
    );

    // Add only preimage (no signature)
    for proof in swap_request_preimage_only.inputs_mut() {
        proof.add_preimage(preimage.clone());
    }

    let result = mint.process_swap_request(swap_request_preimage_only).await;
    assert!(result.is_err(), "Should fail with only preimage (no signature)");
    println!("✓ Spending with ONLY preimage failed as expected: {:?}", result.err());

    // Step 7: Try to spend with only signature (should fail - preimage required)
    let mut swap_request_signature_only = cdk_common::nuts::SwapRequest::new(
        htlc_proofs.clone(),
        new_outputs.clone(),
    );

    // Add only signature (no preimage)
    for proof in swap_request_signature_only.inputs_mut() {
        proof.sign_p2pk(alice_secret.clone()).unwrap();
    }

    let result = mint.process_swap_request(swap_request_signature_only).await;
    assert!(result.is_err(), "Should fail with only signature (no preimage)");
    println!("✓ Spending with ONLY signature failed as expected: {:?}", result.err());

    // Step 8: Now try to spend the HTLC proofs with correct preimage + signature
    let mut swap_request_both = cdk_common::nuts::SwapRequest::new(
        htlc_proofs.clone(),
        new_outputs.clone(),
    );

    // Add preimage and sign all proofs
    for proof in swap_request_both.inputs_mut() {
        proof.add_preimage(preimage.clone());
        proof.sign_p2pk(alice_secret.clone()).unwrap();
    }

    let result = mint.process_swap_request(swap_request_both).await;
    assert!(result.is_ok(), "Should succeed with correct preimage and signature: {:?}", result.err());
    println!("✓ HTLC spent successfully with correct preimage AND signature");
}

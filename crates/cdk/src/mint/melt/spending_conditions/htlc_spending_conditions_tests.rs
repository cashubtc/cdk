#![cfg(test)]
//! HTLC (NUT-14) tests for melt functionality
//!
//! These tests verify that the mint correctly validates HTLC spending conditions
//! during melt operations, including:
//! - Hash preimage verification
//! - Signature validation

use cdk_common::nuts::{SpendingConditions, Conditions, SigFlag};
use cdk_common::Amount;
use cdk_common::dhke::construct_proofs;

use crate::mint::swap::spending_conditions::test_helpers::{TestMintHelper, create_test_keypair, create_test_hash_and_preimage, unzip3};

/// Test: HTLC requiring preimage and one signature
///
/// Creates HTLC-locked proofs and verifies:
/// 1. Melting with only preimage fails (signature required)
/// 2. Melting with only signature fails (preimage required)
/// 3. Melting with both preimage and signature succeeds
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
    let swap_request = cdk_common::SwapRequest::new(
        input_proofs.clone(),
        htlc_outputs.clone(),
    );
    let swap_response = mint
        .process_swap_request(swap_request)
        .await
        .expect("Failed to swap for HTLC proofs");
    println!("Swap successful! Got BlindSignatures for our HTLC outputs");

    // Step 5: Construct the HTLC proofs
    let htlc_proofs = construct_proofs(
        swap_response.signatures.clone(),
        blinding_factors.clone(),
        secrets.clone(),
        &test_mint.public_keys_of_the_active_sat_keyset,
    ).unwrap();

    let proof_amounts: Vec<String> = htlc_proofs.iter().map(|p| p.amount.to_string()).collect();
    println!("Constructed {} HTLC proof(s) [{}]", htlc_proofs.len(), proof_amounts.join("+"));

    // Step 6: Try to melt with only preimage (should fail - signature required)
    use cdk_common::quote_id::QuoteId;
    use cdk_common::SpendingConditionVerification;
    use uuid::Uuid;

    let fake_quote_id = QuoteId::UUID(Uuid::new_v4());

    let mut proofs_preimage_only = htlc_proofs.clone();

    // Add only preimage (no signature)
    for proof in proofs_preimage_only.iter_mut() {
        proof.add_preimage(preimage.clone());
    }

    let melt_request_preimage_only = cdk_common::MeltRequest::new(
        fake_quote_id.clone(),
        proofs_preimage_only.into(),
        None,
    );

    let result = melt_request_preimage_only.verify_spending_conditions();
    assert!(result.is_err(), "Should fail with only preimage (no signature)");
    println!("✓ Melting with ONLY preimage failed as expected");

    // Step 7: Try to melt with only signature (should fail - preimage required)
    let mut proofs_signature_only = htlc_proofs.clone();

    // Add only signature (no preimage)
    for proof in proofs_signature_only.iter_mut() {
        proof.sign_p2pk(alice_secret.clone()).unwrap();
    }

    let melt_request_signature_only = cdk_common::MeltRequest::new(
        fake_quote_id.clone(),
        proofs_signature_only.into(),
        None,
    );

    let result = melt_request_signature_only.verify_spending_conditions();
    assert!(result.is_err(), "Should fail with only signature (no preimage)");
    println!("✓ Melting with ONLY signature failed as expected");

    // Step 8: Now try to melt the HTLC proofs with correct preimage + signature
    let mut proofs_both = htlc_proofs.clone();

    // Add preimage and sign all proofs
    for proof in proofs_both.iter_mut() {
        proof.add_preimage(preimage.clone());
        proof.sign_p2pk(alice_secret.clone()).unwrap();
    }

    let melt_request_both = cdk_common::MeltRequest::new(
        fake_quote_id,
        proofs_both.into(),
        None,
    );

    melt_request_both.verify_spending_conditions().unwrap();
    println!("✓ HTLC melt verified successfully with correct preimage AND signature");
}

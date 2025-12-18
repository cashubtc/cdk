//! HTLC (NUT-14) tests for swap functionality
//!
//! These tests verify that the mint correctly validates HTLC spending conditions
//! during swap operations, including:
//! - Hash preimage verification
//! - Locktime enforcement
//! - Refund keys
//! - Signature validation

use cdk_common::nuts::{Conditions, SigFlag, SpendingConditions};
use cdk_common::Amount;

use crate::test_helpers::nut10::{
    create_test_hash_and_preimage, create_test_keypair, unzip3, TestMintHelper,
};

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
        }),
    )
    .unwrap();
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
    println!(
        "Created {} HTLC outputs locked to alice with hash",
        htlc_outputs.len()
    );

    // Step 4: Swap regular proofs for HTLC proofs (no signature needed on inputs)
    let swap_request =
        cdk_common::nuts::SwapRequest::new(input_proofs.clone(), htlc_outputs.clone());
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
    )
    .unwrap();

    let proof_amounts: Vec<String> = htlc_proofs.iter().map(|p| p.amount.to_string()).collect();
    println!(
        "Constructed {} HTLC proof(s) [{}]",
        htlc_proofs.len(),
        proof_amounts.join("+")
    );

    // Step 6: Try to spend with only preimage (should fail - signature required)
    use crate::test_helpers::mint::create_test_blinded_messages;
    let (new_outputs, _) = create_test_blinded_messages(mint, input_amount)
        .await
        .unwrap();
    let mut swap_request_preimage_only =
        cdk_common::nuts::SwapRequest::new(htlc_proofs.clone(), new_outputs.clone());

    // Add only preimage (no signature)
    for proof in swap_request_preimage_only.inputs_mut() {
        proof.add_preimage(preimage.clone());
    }

    let result = mint.process_swap_request(swap_request_preimage_only).await;
    assert!(
        result.is_err(),
        "Should fail with only preimage (no signature)"
    );
    println!(
        "✓ Spending with ONLY preimage failed as expected: {:?}",
        result.err()
    );

    // Step 7: Try to spend with only signature (should fail - preimage required)
    let mut swap_request_signature_only =
        cdk_common::nuts::SwapRequest::new(htlc_proofs.clone(), new_outputs.clone());

    // Add only signature (no preimage)
    for proof in swap_request_signature_only.inputs_mut() {
        proof.sign_p2pk(alice_secret.clone()).unwrap();
    }

    let result = mint.process_swap_request(swap_request_signature_only).await;
    assert!(
        result.is_err(),
        "Should fail with only signature (no preimage)"
    );
    println!(
        "✓ Spending with ONLY signature failed as expected: {:?}",
        result.err()
    );

    // Step 8: Now try to spend the HTLC proofs with correct preimage + signature
    let mut swap_request_both =
        cdk_common::nuts::SwapRequest::new(htlc_proofs.clone(), new_outputs.clone());

    // Add preimage and sign all proofs
    for proof in swap_request_both.inputs_mut() {
        proof.add_preimage(preimage.clone());
        proof.sign_p2pk(alice_secret.clone()).unwrap();
    }

    let result = mint.process_swap_request(swap_request_both).await;
    assert!(
        result.is_ok(),
        "Should succeed with correct preimage and signature: {:?}",
        result.err()
    );
    println!("✓ HTLC spent successfully with correct preimage AND signature");
}

/// Test: HTLC with wrong preimage
///
/// Verifies that providing an incorrect preimage fails even with correct signature
#[tokio::test]
async fn test_htlc_wrong_preimage() {
    let test_mint = TestMintHelper::new().await.unwrap();
    let mint = test_mint.mint();

    let (alice_secret, alice_pubkey) = create_test_keypair();
    let (hash, _correct_preimage) = create_test_hash_and_preimage();

    // Mint regular proofs and swap for HTLC proofs
    let input_amount = Amount::from(10);
    let input_proofs = test_mint.mint_proofs(input_amount).await.unwrap();

    let spending_conditions = SpendingConditions::new_htlc_hash(
        &hash,
        Some(Conditions {
            locktime: None,
            pubkeys: Some(vec![alice_pubkey]),
            refund_keys: None,
            num_sigs: None,
            sig_flag: SigFlag::default(),
            num_sigs_refund: None,
        }),
    )
    .unwrap();

    let split_amounts = test_mint.split_amount(input_amount).unwrap();
    let (htlc_outputs, blinding_factors, secrets) = unzip3(
        split_amounts
            .iter()
            .map(|&amt| test_mint.create_blinded_message(amt, &spending_conditions))
            .collect(),
    );

    let swap_request =
        cdk_common::nuts::SwapRequest::new(input_proofs.clone(), htlc_outputs.clone());
    let swap_response = mint.process_swap_request(swap_request).await.unwrap();

    use cdk_common::dhke::construct_proofs;
    let htlc_proofs = construct_proofs(
        swap_response.signatures.clone(),
        blinding_factors.clone(),
        secrets.clone(),
        &test_mint.public_keys_of_the_active_sat_keyset,
    )
    .unwrap();

    // Try to spend with WRONG preimage (but correct signature)
    use crate::test_helpers::mint::create_test_blinded_messages;
    let (new_outputs, _) = create_test_blinded_messages(mint, input_amount)
        .await
        .unwrap();
    let mut swap_request =
        cdk_common::nuts::SwapRequest::new(htlc_proofs.clone(), new_outputs.clone());

    let wrong_preimage = "this_is_the_wrong_preimage";
    for proof in swap_request.inputs_mut() {
        proof.add_preimage(wrong_preimage.to_string());
        proof.sign_p2pk(alice_secret.clone()).unwrap();
    }

    let result = mint.process_swap_request(swap_request).await;
    assert!(result.is_err(), "Should fail with wrong preimage");
    println!(
        "✓ HTLC with wrong preimage failed as expected: {:?}",
        result.err()
    );
}

/// Test: HTLC locktime after expiry (refund path)
///
/// Verifies that after locktime expires, refund keys can spend without preimage
#[tokio::test]
async fn test_htlc_locktime_after_expiry() {
    let test_mint = TestMintHelper::new().await.unwrap();
    let mint = test_mint.mint();

    let (_alice_secret, alice_pubkey) = create_test_keypair();
    let (bob_secret, bob_pubkey) = create_test_keypair();
    let (hash, _preimage) = create_test_hash_and_preimage();

    // Create HTLC with locktime in the PAST (already expired) and Bob as refund key
    let past_locktime = cdk_common::util::unix_time() - 1000;

    let input_amount = Amount::from(10);
    let input_proofs = test_mint.mint_proofs(input_amount).await.unwrap();

    let spending_conditions = SpendingConditions::new_htlc_hash(
        &hash,
        Some(Conditions {
            locktime: Some(past_locktime),
            pubkeys: Some(vec![alice_pubkey]),
            refund_keys: Some(vec![bob_pubkey]),
            num_sigs: None,
            sig_flag: SigFlag::default(),
            num_sigs_refund: None,
        }),
    )
    .unwrap();

    let split_amounts = test_mint.split_amount(input_amount).unwrap();
    let (htlc_outputs, blinding_factors, secrets) = unzip3(
        split_amounts
            .iter()
            .map(|&amt| test_mint.create_blinded_message(amt, &spending_conditions))
            .collect(),
    );

    let swap_request =
        cdk_common::nuts::SwapRequest::new(input_proofs.clone(), htlc_outputs.clone());
    let swap_response = mint.process_swap_request(swap_request).await.unwrap();

    use cdk_common::dhke::construct_proofs;
    let htlc_proofs = construct_proofs(
        swap_response.signatures.clone(),
        blinding_factors.clone(),
        secrets.clone(),
        &test_mint.public_keys_of_the_active_sat_keyset,
    )
    .unwrap();

    // After locktime, Bob (refund key) can spend WITHOUT preimage
    use crate::test_helpers::mint::create_test_blinded_messages;
    let (new_outputs, _) = create_test_blinded_messages(mint, input_amount)
        .await
        .unwrap();
    let mut swap_request =
        cdk_common::nuts::SwapRequest::new(htlc_proofs.clone(), new_outputs.clone());

    // Bob signs (no preimage needed after locktime)
    // Note: Must call add_preimage first (even with empty string) to create HTLC witness,
    // otherwise sign_p2pk creates P2PK witness instead
    for proof in swap_request.inputs_mut() {
        proof.add_preimage(String::new()); // Empty preimage for refund path
        proof.sign_p2pk(bob_secret.clone()).unwrap();
    }

    let result = mint.process_swap_request(swap_request).await;
    assert!(
        result.is_ok(),
        "Bob should be able to spend after locktime without preimage: {:?}",
        result.err()
    );
    println!("✓ HTLC spent by refund key after locktime (no preimage needed)");
}

/// Test: HTLC with multisig (preimage + 2-of-3 signatures)
///
/// Verifies that HTLC can require preimage AND multiple signatures
#[tokio::test]
async fn test_htlc_multisig_2of3() {
    let test_mint = TestMintHelper::new().await.unwrap();
    let mint = test_mint.mint();

    let (alice_secret, alice_pubkey) = create_test_keypair();
    let (bob_secret, bob_pubkey) = create_test_keypair();
    let (_charlie_secret, charlie_pubkey) = create_test_keypair();
    let (hash, preimage) = create_test_hash_and_preimage();

    // Create HTLC requiring preimage + 2-of-3 signatures (Alice, Bob, Charlie)
    let input_amount = Amount::from(10);
    let input_proofs = test_mint.mint_proofs(input_amount).await.unwrap();

    let spending_conditions = SpendingConditions::new_htlc_hash(
        &hash,
        Some(Conditions {
            locktime: None,
            pubkeys: Some(vec![alice_pubkey, bob_pubkey, charlie_pubkey]),
            refund_keys: None,
            num_sigs: Some(2), // Require 2 of 3
            sig_flag: SigFlag::default(),
            num_sigs_refund: None,
        }),
    )
    .unwrap();

    let split_amounts = test_mint.split_amount(input_amount).unwrap();
    let (htlc_outputs, blinding_factors, secrets) = unzip3(
        split_amounts
            .iter()
            .map(|&amt| test_mint.create_blinded_message(amt, &spending_conditions))
            .collect(),
    );

    let swap_request =
        cdk_common::nuts::SwapRequest::new(input_proofs.clone(), htlc_outputs.clone());
    let swap_response = mint.process_swap_request(swap_request).await.unwrap();

    use cdk_common::dhke::construct_proofs;
    let htlc_proofs = construct_proofs(
        swap_response.signatures.clone(),
        blinding_factors.clone(),
        secrets.clone(),
        &test_mint.public_keys_of_the_active_sat_keyset,
    )
    .unwrap();

    // Try with preimage + only 1 signature (should fail - need 2)
    use crate::test_helpers::mint::create_test_blinded_messages;
    let (new_outputs, _) = create_test_blinded_messages(mint, input_amount)
        .await
        .unwrap();
    let mut swap_request_one_sig =
        cdk_common::nuts::SwapRequest::new(htlc_proofs.clone(), new_outputs.clone());

    for proof in swap_request_one_sig.inputs_mut() {
        proof.add_preimage(preimage.clone());
        proof.sign_p2pk(alice_secret.clone()).unwrap(); // Only Alice signs
    }

    let result = mint.process_swap_request(swap_request_one_sig).await;
    assert!(
        result.is_err(),
        "Should fail with only 1 signature (need 2)"
    );
    println!("✓ HTLC with 1-of-3 signatures failed as expected");

    // Now with preimage + 2 signatures (Alice and Bob) - should succeed
    let mut swap_request_two_sigs =
        cdk_common::nuts::SwapRequest::new(htlc_proofs.clone(), new_outputs.clone());

    for proof in swap_request_two_sigs.inputs_mut() {
        proof.add_preimage(preimage.clone());
        proof.sign_p2pk(alice_secret.clone()).unwrap();
        proof.sign_p2pk(bob_secret.clone()).unwrap();
    }

    let result = mint.process_swap_request(swap_request_two_sigs).await;
    assert!(
        result.is_ok(),
        "Should succeed with preimage + 2-of-3 signatures: {:?}",
        result.err()
    );
    println!("✓ HTLC spent with preimage + 2-of-3 signatures");
}

/// Test: HTLC receiver path still works after locktime (NUT-14 compliance)
///
/// Per NUT-14: "This pathway is ALWAYS available to the receivers, as possession
/// of the preimage confirms performance of the Sender's wishes."
///
/// This test verifies that even after locktime has passed, the receiver can still
/// spend using the preimage + pubkeys path (not just the refund path).
#[tokio::test]
async fn test_htlc_receiver_path_after_locktime() {
    let test_mint = TestMintHelper::new().await.unwrap();
    let mint = test_mint.mint();

    let (alice_secret, alice_pubkey) = create_test_keypair();
    let (_bob_secret, bob_pubkey) = create_test_keypair();
    let (hash, preimage) = create_test_hash_and_preimage();

    // Create HTLC with locktime in the PAST (already expired)
    // Alice is the receiver (pubkeys), Bob is the refund key
    let past_locktime = cdk_common::util::unix_time() - 1000;

    let input_amount = Amount::from(10);
    let input_proofs = test_mint.mint_proofs(input_amount).await.unwrap();

    let spending_conditions = SpendingConditions::new_htlc_hash(
        &hash,
        Some(Conditions {
            locktime: Some(past_locktime),
            pubkeys: Some(vec![alice_pubkey]),
            refund_keys: Some(vec![bob_pubkey]),
            num_sigs: None,
            sig_flag: SigFlag::default(),
            num_sigs_refund: None,
        }),
    )
    .unwrap();

    let split_amounts = test_mint.split_amount(input_amount).unwrap();
    let (htlc_outputs, blinding_factors, secrets) = unzip3(
        split_amounts
            .iter()
            .map(|&amt| test_mint.create_blinded_message(amt, &spending_conditions))
            .collect(),
    );

    let swap_request =
        cdk_common::nuts::SwapRequest::new(input_proofs.clone(), htlc_outputs.clone());
    let swap_response = mint.process_swap_request(swap_request).await.unwrap();

    use cdk_common::dhke::construct_proofs;
    let htlc_proofs = construct_proofs(
        swap_response.signatures.clone(),
        blinding_factors.clone(),
        secrets.clone(),
        &test_mint.public_keys_of_the_active_sat_keyset,
    )
    .unwrap();

    // Even though locktime has passed, Alice (receiver) should STILL be able to spend
    // using the preimage + her signature (receiver path is ALWAYS available per NUT-14)
    use crate::test_helpers::mint::create_test_blinded_messages;
    let (new_outputs, _) = create_test_blinded_messages(mint, input_amount)
        .await
        .unwrap();
    let mut swap_request =
        cdk_common::nuts::SwapRequest::new(htlc_proofs.clone(), new_outputs.clone());

    // Alice provides preimage and signs (receiver path)
    for proof in swap_request.inputs_mut() {
        proof.add_preimage(preimage.clone());
        proof.sign_p2pk(alice_secret.clone()).unwrap();
    }

    let result = mint.process_swap_request(swap_request).await;
    assert!(
        result.is_ok(),
        "Receiver should be able to spend with preimage even after locktime: {:?}",
        result.err()
    );
    println!("✓ HTLC receiver path works after locktime (NUT-14 compliant)");
}

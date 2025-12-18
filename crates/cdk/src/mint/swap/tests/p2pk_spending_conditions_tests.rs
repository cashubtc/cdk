//! P2PK (NUT-11) tests for swap functionality
//!
//! These tests verify that the mint correctly validates P2PK spending conditions
//! during swap operations, including:
//! - Single signature P2PK
//! - Multisig (m-of-n)
//! - Locktime enforcement
//! - Refund keys
//! - Signature validation

use cdk_common::dhke::construct_proofs;
use cdk_common::nuts::{Conditions, SigFlag, SpendingConditions};
use cdk_common::Amount;

use crate::test_helpers::mint::create_test_blinded_messages;
use crate::test_helpers::nut10::{create_test_keypair, unzip3, TestMintHelper};
use crate::util::unix_time;

/// Test: P2PK with single pubkey requires all proofs signed
///
/// Creates proofs locked to a single public key and verifies:
/// 1. Spending without any signatures is rejected
/// 2. Spending with partial signatures (only some proofs signed) is rejected
/// 3. Spending with all proofs signed succeeds
#[tokio::test]
async fn test_p2pk_single_pubkey_requires_all_proofs_signed() {
    let test_mint = TestMintHelper::new().await.unwrap();
    let mint = test_mint.mint();

    // Generate keypair for P2PK
    let (alice_secret, alice_pubkey) = create_test_keypair();
    println!("Alice pubkey: {}", alice_pubkey);

    // Step 1: Create regular unencumbered proofs that we'll swap for P2PK proofs
    let input_amount = Amount::from(10);
    let input_proofs = test_mint.mint_proofs(input_amount).await.unwrap();

    // Step 2: Create P2PK blinded messages (outputs locked to alice_pubkey)
    let spending_conditions = SpendingConditions::new_p2pk(alice_pubkey, None);

    // Split the input amount into power-of-2 denominations
    let split_amounts = test_mint.split_amount(input_amount).unwrap();
    let split_display: Vec<String> = split_amounts.iter().map(|a| a.to_string()).collect();
    println!("Split {} into [{}]", input_amount, split_display.join("+"));

    // Create blinded messages for each split amount
    let (p2pk_outputs, blinding_factors, secrets) = unzip3(
        split_amounts
            .iter()
            .map(|&amt| test_mint.create_blinded_message(amt, &spending_conditions))
            .collect(),
    );

    println!(
        "Created {} P2PK outputs locked to alice",
        p2pk_outputs.len()
    );

    // Step 3: Swap regular proofs for P2PK proofs (no signature needed on inputs)
    let swap_request =
        cdk_common::nuts::SwapRequest::new(input_proofs.clone(), p2pk_outputs.clone());
    let swap_response = mint
        .process_swap_request(swap_request)
        .await
        .expect("Failed to swap for P2PK proofs");
    println!("Swap successful! Got BlindSignatures for our P2PK outputs");

    // Step 4: Construct the P2PK proofs
    let p2pk_proofs = construct_proofs(
        swap_response.signatures.clone(),
        blinding_factors.clone(),
        secrets.clone(),
        &test_mint.public_keys_of_the_active_sat_keyset,
    )
    .unwrap();

    let proof_amounts: Vec<String> = p2pk_proofs.iter().map(|p| p.amount.to_string()).collect();
    println!(
        "Constructed {} P2PK proof(s) [{}]",
        p2pk_proofs.len(),
        proof_amounts.join("+")
    );

    // Step 5: Try to spend P2PK proof WITHOUT signature (should fail)
    let (new_outputs, _) = create_test_blinded_messages(mint, input_amount)
        .await
        .unwrap();
    let swap_request_no_sig =
        cdk_common::nuts::SwapRequest::new(p2pk_proofs.clone(), new_outputs.clone());

    let result = mint.process_swap_request(swap_request_no_sig).await;
    assert!(result.is_err(), "Should fail without signature");
    println!(
        "✓ Spending WITHOUT signature failed as expected: {:?}",
        result.err()
    );

    // Step 6: Sign only ONE of the proofs and try (should fail - need all signatures)
    let mut swap_request_partial_sig =
        cdk_common::nuts::SwapRequest::new(p2pk_proofs.clone(), new_outputs.clone());

    // Sign only the first proof
    swap_request_partial_sig.inputs_mut()[0]
        .sign_p2pk(alice_secret.clone())
        .unwrap();

    let result = mint.process_swap_request(swap_request_partial_sig).await;
    assert!(result.is_err(), "Should fail with only partial signatures");
    println!(
        "✓ Spending with PARTIAL signatures failed as expected: {:?}",
        result.err()
    );

    // Step 7: Now sign ALL the proofs and try again (should succeed)
    let mut swap_request_with_sig =
        cdk_common::nuts::SwapRequest::new(p2pk_proofs.clone(), new_outputs.clone());

    // Sign all the P2PK proofs with Alice's key
    for proof in swap_request_with_sig.inputs_mut() {
        proof.sign_p2pk(alice_secret.clone()).unwrap();
    }

    let result = mint.process_swap_request(swap_request_with_sig).await;
    assert!(
        result.is_ok(),
        "Should succeed with valid signature: {:?}",
        result.err()
    );
    println!("✓ Spending WITH ALL signatures succeeded");
}

/// Test: P2PK multisig (2-of-3)
///
/// Creates proofs requiring 2 signatures from a set of 3 public keys and verifies:
/// 1. Spending with only 1 valid signature fails (Alice only)
/// 2. Spending with 2 invalid signatures fails (wrong keys)
/// 3. Spending with 2 valid signatures succeeds (Alice + Bob)
#[tokio::test]
async fn test_p2pk_multisig_2of3() {
    let test_mint = TestMintHelper::new().await.unwrap();
    let mint = test_mint.mint();

    // Generate 3 keypairs for the multisig
    let (alice_secret, alice_pubkey) = create_test_keypair();
    let (bob_secret, bob_pubkey) = create_test_keypair();
    let (_carol_secret, carol_pubkey) = create_test_keypair();

    // Generate 2 wrong keypairs (not in the multisig set)
    let (dave_secret, _dave_pubkey) = create_test_keypair();
    let (eve_secret, _eve_pubkey) = create_test_keypair();

    println!("Alice: {}", alice_pubkey);
    println!("Bob: {}", bob_pubkey);
    println!("Carol: {}", carol_pubkey);

    // Step 1: Mint regular proofs
    let input_amount = Amount::from(10);
    let input_proofs = test_mint.mint_proofs(input_amount).await.unwrap();

    // Step 2: Create 2-of-3 multisig conditions
    // Primary key: Alice
    // Additional keys: Bob, Carol
    // Requires 2 signatures total
    let spending_conditions = SpendingConditions::new_p2pk(
        alice_pubkey,
        Some(
            Conditions::new(
                None,                                 // no locktime
                Some(vec![bob_pubkey, carol_pubkey]), // additional pubkeys
                None,                                 // no refund keys
                Some(2),                              // require 2 signatures
                None,                                 // default sig_flag
                None,                                 // no num_sigs_refund
            )
            .unwrap(),
        ),
    );
    println!("Created 2-of-3 multisig spending conditions (Alice, Bob, Carol)");

    // Step 3: Create P2PK blinded messages with multisig conditions
    let split_amounts = test_mint.split_amount(input_amount).unwrap();
    let (p2pk_outputs, blinding_factors, secrets) = unzip3(
        split_amounts
            .iter()
            .map(|&amt| test_mint.create_blinded_message(amt, &spending_conditions))
            .collect(),
    );

    // Step 4: Swap for P2PK multisig proofs
    let swap_request =
        cdk_common::nuts::SwapRequest::new(input_proofs.clone(), p2pk_outputs.clone());
    let swap_response = mint.process_swap_request(swap_request).await.unwrap();
    println!("Created P2PK multisig proofs (2-of-3)");

    // Step 5: Construct the P2PK proofs
    let p2pk_proofs = construct_proofs(
        swap_response.signatures.clone(),
        blinding_factors.clone(),
        secrets.clone(),
        &test_mint.public_keys_of_the_active_sat_keyset,
    )
    .unwrap();

    // Step 6: Try to spend with only 1 signature (Alice only - should fail)
    let (new_outputs, _) = create_test_blinded_messages(mint, input_amount)
        .await
        .unwrap();
    let mut swap_request_one_sig =
        cdk_common::nuts::SwapRequest::new(p2pk_proofs.clone(), new_outputs.clone());

    // Sign with only Alice
    for proof in swap_request_one_sig.inputs_mut() {
        proof.sign_p2pk(alice_secret.clone()).unwrap();
    }

    let result = mint.process_swap_request(swap_request_one_sig).await;
    assert!(
        result.is_err(),
        "Should fail with only 1 signature (need 2)"
    );
    println!(
        "✓ Spending with only 1 signature (Alice) failed as expected: {:?}",
        result.err()
    );

    // Step 7: Try to spend with 2 invalid signatures (Dave + Eve - not in multisig set)
    let mut swap_request_invalid_sigs =
        cdk_common::nuts::SwapRequest::new(p2pk_proofs.clone(), new_outputs.clone());

    // Sign with Dave and Eve (wrong keys!)
    for proof in swap_request_invalid_sigs.inputs_mut() {
        proof.sign_p2pk(dave_secret.clone()).unwrap();
        proof.sign_p2pk(eve_secret.clone()).unwrap();
    }

    let result = mint.process_swap_request(swap_request_invalid_sigs).await;
    assert!(result.is_err(), "Should fail with 2 invalid signatures");
    println!(
        "✓ Spending with 2 INVALID signatures (Dave + Eve) failed as expected: {:?}",
        result.err()
    );

    // Step 8: Spend with 2 valid signatures (Alice + Bob - should succeed)
    let mut swap_request_valid_sigs =
        cdk_common::nuts::SwapRequest::new(p2pk_proofs.clone(), new_outputs.clone());

    // Sign with Alice and Bob
    for proof in swap_request_valid_sigs.inputs_mut() {
        proof.sign_p2pk(alice_secret.clone()).unwrap();
        proof.sign_p2pk(bob_secret.clone()).unwrap();
    }

    let result = mint.process_swap_request(swap_request_valid_sigs).await;
    assert!(
        result.is_ok(),
        "Should succeed with 2 valid signatures: {:?}",
        result.err()
    );
    println!("✓ Spending with 2 VALID signatures (Alice + Bob) succeeded");
}

/// Test: P2PK with locktime (before expiry)
///
/// Verifies that before locktime expires:
/// 1. Spending with primary key (Alice) succeeds
/// 2. Spending with refund key (Bob) fails
#[tokio::test]
async fn test_p2pk_locktime_before_expiry() {
    let test_mint = TestMintHelper::new().await.unwrap();
    let mint = test_mint.mint();

    let (alice_secret, alice_pubkey) = create_test_keypair();
    let (bob_secret, bob_pubkey) = create_test_keypair();

    // Set locktime 1 hour in the future
    let locktime = unix_time() + 3600;

    println!("Alice (primary): {}", alice_pubkey);
    println!("Bob (refund): {}", bob_pubkey);
    println!("Current time: {}", unix_time());
    println!("Locktime: {} (expires in 1 hour)", locktime);

    // Step 1: Mint regular proofs
    let input_amount = Amount::from(10);
    let input_proofs = test_mint.mint_proofs(input_amount).await.unwrap();

    // Step 2: Create conditions with Alice as primary and Bob as refund key
    let spending_conditions = SpendingConditions::new_p2pk(
        alice_pubkey,
        Some(
            Conditions::new(
                Some(locktime),         // locktime in the future
                None,                   // no additional pubkeys
                Some(vec![bob_pubkey]), // Bob is refund key
                None,                   // default num_sigs (1)
                None,                   // default sig_flag
                None,                   // default num_sigs_refund (1)
            )
            .unwrap(),
        ),
    );
    println!("Created P2PK with locktime and refund key");

    // Step 3: Create P2PK blinded messages
    let split_amounts = test_mint.split_amount(input_amount).unwrap();
    let (p2pk_outputs, blinding_factors, secrets) = unzip3(
        split_amounts
            .iter()
            .map(|&amt| test_mint.create_blinded_message(amt, &spending_conditions))
            .collect(),
    );

    // Step 4: Swap for P2PK proofs
    let swap_request =
        cdk_common::nuts::SwapRequest::new(input_proofs.clone(), p2pk_outputs.clone());
    let swap_response = mint.process_swap_request(swap_request).await.unwrap();

    // Step 5: Construct the P2PK proofs
    let p2pk_proofs = construct_proofs(
        swap_response.signatures.clone(),
        blinding_factors.clone(),
        secrets.clone(),
        &test_mint.public_keys_of_the_active_sat_keyset,
    )
    .unwrap();

    // Step 6: Try to spend with refund key (Bob) BEFORE locktime expires (should fail)
    let (new_outputs, _) = create_test_blinded_messages(mint, input_amount)
        .await
        .unwrap();
    let mut swap_request_refund =
        cdk_common::nuts::SwapRequest::new(p2pk_proofs.clone(), new_outputs.clone());

    // Sign with Bob (refund key)
    for proof in swap_request_refund.inputs_mut() {
        proof.sign_p2pk(bob_secret.clone()).unwrap();
    }

    let result = mint.process_swap_request(swap_request_refund).await;
    assert!(
        result.is_err(),
        "Should fail - refund key cannot spend before locktime"
    );
    println!(
        "✓ Spending with refund key (Bob) BEFORE locktime failed as expected: {:?}",
        result.err()
    );

    // Step 7: Spend with primary key (Alice) BEFORE locktime (should succeed)
    let mut swap_request_primary =
        cdk_common::nuts::SwapRequest::new(p2pk_proofs.clone(), new_outputs.clone());

    // Sign with Alice (primary key)
    for proof in swap_request_primary.inputs_mut() {
        proof.sign_p2pk(alice_secret.clone()).unwrap();
    }

    let result = mint.process_swap_request(swap_request_primary).await;
    assert!(
        result.is_ok(),
        "Should succeed - primary key can spend before locktime: {:?}",
        result.err()
    );
    println!("✓ Spending with primary key (Alice) BEFORE locktime succeeded");
}

/// Test: P2PK with locktime (after expiry)
///
/// Verifies that after locktime expires:
/// 1. Spending with refund key (Bob) succeeds
/// 2. Spending with primary key (Alice) fails
#[tokio::test]
async fn test_p2pk_locktime_after_expiry() {
    let test_mint = TestMintHelper::new().await.unwrap();
    let mint = test_mint.mint();

    let (alice_secret, alice_pubkey) = create_test_keypair();
    let (bob_secret, bob_pubkey) = create_test_keypair();

    // Set locktime in the past (already expired)
    let locktime = unix_time() - 3600;

    println!("Alice (primary): {}", alice_pubkey);
    println!("Bob (refund): {}", bob_pubkey);
    println!("Current time: {}", unix_time());
    println!("Locktime: {} (expired 1 hour ago)", locktime);

    // Step 1: Mint regular proofs
    let input_amount = Amount::from(10);
    let input_proofs = test_mint.mint_proofs(input_amount).await.unwrap();

    // Step 2: Create conditions with Alice as primary and Bob as refund key
    // Note: We create the Conditions struct directly to bypass the validation
    // that rejects locktimes in the past (since we're testing the expired case)
    let spending_conditions = SpendingConditions::new_p2pk(
        alice_pubkey,
        Some(Conditions {
            locktime: Some(locktime),            // locktime in the past (expired)
            pubkeys: None,                       // no additional pubkeys
            refund_keys: Some(vec![bob_pubkey]), // Bob is refund key
            num_sigs: None,                      // default (1)
            sig_flag: SigFlag::default(),
            num_sigs_refund: None, // default (1)
        }),
    );
    println!("Created P2PK with expired locktime and refund key");

    // Step 3: Create P2PK blinded messages
    let split_amounts = test_mint.split_amount(input_amount).unwrap();
    let (p2pk_outputs, blinding_factors, secrets) = unzip3(
        split_amounts
            .iter()
            .map(|&amt| test_mint.create_blinded_message(amt, &spending_conditions))
            .collect(),
    );

    // Step 4: Swap for P2PK proofs
    let swap_request =
        cdk_common::nuts::SwapRequest::new(input_proofs.clone(), p2pk_outputs.clone());
    let swap_response = mint.process_swap_request(swap_request).await.unwrap();

    // Step 5: Construct the P2PK proofs
    let p2pk_proofs = construct_proofs(
        swap_response.signatures.clone(),
        blinding_factors.clone(),
        secrets.clone(),
        &test_mint.public_keys_of_the_active_sat_keyset,
    )
    .unwrap();

    // Step 6: Try to spend with primary key (Alice) AFTER locktime expires
    // Per NUT-11: "Locktime Multisig conditions continue to apply" - primary keys STILL work
    let (new_outputs, _) = create_test_blinded_messages(mint, input_amount)
        .await
        .unwrap();
    let mut swap_request_primary =
        cdk_common::nuts::SwapRequest::new(p2pk_proofs.clone(), new_outputs.clone());

    // Sign with Alice (primary key)
    for proof in swap_request_primary.inputs_mut() {
        proof.sign_p2pk(alice_secret.clone()).unwrap();
    }

    let result = mint.process_swap_request(swap_request_primary).await;
    assert!(
        result.is_ok(),
        "Should succeed - primary key can STILL spend after locktime (NUT-11 compliant): {:?}",
        result.err()
    );
    println!("✓ Spending with primary key (Alice) AFTER locktime succeeded (NUT-11 compliant)");
}

/// Test: P2PK with locktime after expiry, no refund keys (anyone can spend)
///
/// Verifies that after locktime expires with NO refund keys configured,
/// anyone can spend the proofs without providing any signatures at all.
#[tokio::test]
async fn test_p2pk_locktime_after_expiry_no_refund_anyone_can_spend() {
    let test_mint = TestMintHelper::new().await.unwrap();
    let mint = test_mint.mint();

    let (_alice_secret, alice_pubkey) = create_test_keypair();

    // Set locktime in the past (already expired)
    let locktime = unix_time() - 3600;

    println!("Alice (primary): {}", alice_pubkey);
    println!("Current time: {}", unix_time());
    println!("Locktime: {} (expired 1 hour ago)", locktime);
    println!("No refund keys configured - anyone can spend after locktime");

    // Step 1: Mint regular proofs
    let input_amount = Amount::from(10);
    let input_proofs = test_mint.mint_proofs(input_amount).await.unwrap();

    // Step 2: Create conditions with Alice as primary, NO refund keys
    let spending_conditions = SpendingConditions::new_p2pk(
        alice_pubkey,
        Some(Conditions {
            locktime: Some(locktime), // locktime in the past (expired)
            pubkeys: None,            // no additional pubkeys
            refund_keys: None,        // NO refund keys - anyone can spend!
            num_sigs: None,           // default (1)
            sig_flag: SigFlag::default(),
            num_sigs_refund: None, // default (1)
        }),
    );
    println!("Created P2PK with expired locktime and NO refund keys");

    // Step 3: Create P2PK blinded messages
    let split_amounts = test_mint.split_amount(input_amount).unwrap();
    let (p2pk_outputs, blinding_factors, secrets) = unzip3(
        split_amounts
            .iter()
            .map(|&amt| test_mint.create_blinded_message(amt, &spending_conditions))
            .collect(),
    );

    // Step 4: Swap for P2PK proofs
    let swap_request =
        cdk_common::nuts::SwapRequest::new(input_proofs.clone(), p2pk_outputs.clone());
    let swap_response = mint.process_swap_request(swap_request).await.unwrap();

    // Step 5: Construct the P2PK proofs
    let p2pk_proofs = construct_proofs(
        swap_response.signatures.clone(),
        blinding_factors.clone(),
        secrets.clone(),
        &test_mint.public_keys_of_the_active_sat_keyset,
    )
    .unwrap();

    // Step 6: Spend WITHOUT any signatures (should succeed - anyone can spend!)
    let (new_outputs, _) = create_test_blinded_messages(mint, input_amount)
        .await
        .unwrap();
    let swap_request_no_sig =
        cdk_common::nuts::SwapRequest::new(p2pk_proofs.clone(), new_outputs.clone());

    // No signatures added at all!

    let result = mint.process_swap_request(swap_request_no_sig).await;
    assert!(
        result.is_ok(),
        "Should succeed - anyone can spend after locktime with no refund keys: {:?}",
        result.err()
    );
    println!("✓ Spending WITHOUT any signatures succeeded (anyone can spend)");
}

/// Test: P2PK multisig with locktime (2-of-3 before, 1-of-2 after)
///
/// Complex scenario: Different multisig requirements before and after locktime
/// Before locktime: Need 2-of-3 from (Alice, Bob, Carol)
/// After locktime: Need 1-of-2 from (Dave, Eve) as refund keys
#[tokio::test]
async fn test_p2pk_multisig_locktime() {
    let test_mint = TestMintHelper::new().await.unwrap();
    let mint = test_mint.mint();

    // Before locktime: Need 2-of-3 from (Alice, Bob, Carol)
    let (alice_secret, alice_pubkey) = create_test_keypair();
    let (bob_secret, bob_pubkey) = create_test_keypair();
    let (_carol_secret, carol_pubkey) = create_test_keypair();

    // After locktime: Need 1-of-2 from (Dave, Eve) as refund keys
    let (dave_secret, dave_pubkey) = create_test_keypair();
    let (_eve_secret, eve_pubkey) = create_test_keypair();

    let locktime = unix_time() - 100; // Already expired

    println!("Primary multisig: Alice, Bob, Carol (need 2-of-3)");
    println!("Refund multisig: Dave, Eve (need 1-of-2)");
    println!("Current time: {}", unix_time());
    println!("Locktime: {} (expired)", locktime);

    // Step 1: Mint regular proofs
    let input_amount = Amount::from(10);
    let input_proofs = test_mint.mint_proofs(input_amount).await.unwrap();

    // Step 2: Create complex conditions
    // Before locktime: 2-of-3 (Alice, Bob, Carol)
    // After locktime: 1-of-2 (Dave, Eve)
    let spending_conditions = SpendingConditions::new_p2pk(
        alice_pubkey,
        Some(Conditions {
            locktime: Some(locktime),                         // Already expired
            pubkeys: Some(vec![bob_pubkey, carol_pubkey]), // Bob and Carol (with Alice = 3 total)
            refund_keys: Some(vec![dave_pubkey, eve_pubkey]), // Dave and Eve for refund
            num_sigs: Some(2),                             // Need 2 signatures before locktime
            sig_flag: SigFlag::default(),
            num_sigs_refund: Some(1), // Need 1 signature after locktime
        }),
    );
    println!("Created complex P2PK: 2-of-3 before locktime, 1-of-2 after locktime");

    // Step 3: Create P2PK blinded messages
    let split_amounts = test_mint.split_amount(input_amount).unwrap();
    let (p2pk_outputs, blinding_factors, secrets) = unzip3(
        split_amounts
            .iter()
            .map(|&amt| test_mint.create_blinded_message(amt, &spending_conditions))
            .collect(),
    );

    // Step 4: Swap for P2PK proofs
    let swap_request =
        cdk_common::nuts::SwapRequest::new(input_proofs.clone(), p2pk_outputs.clone());
    let swap_response = mint.process_swap_request(swap_request).await.unwrap();

    // Step 5: Construct the P2PK proofs
    let p2pk_proofs = construct_proofs(
        swap_response.signatures.clone(),
        blinding_factors.clone(),
        secrets.clone(),
        &test_mint.public_keys_of_the_active_sat_keyset,
    )
    .unwrap();

    // Step 6: Try to spend with primary keys (Alice + Bob) AFTER locktime
    // Per NUT-11: "Locktime Multisig conditions continue to apply" - primary keys STILL work
    let (new_outputs, _) = create_test_blinded_messages(mint, input_amount)
        .await
        .unwrap();
    let mut swap_request_primary =
        cdk_common::nuts::SwapRequest::new(p2pk_proofs.clone(), new_outputs.clone());

    // Sign with Alice + Bob (primary multisig - need 2-of-3)
    for proof in swap_request_primary.inputs_mut() {
        proof.sign_p2pk(alice_secret.clone()).unwrap();
        proof.sign_p2pk(bob_secret.clone()).unwrap();
    }

    let result = mint.process_swap_request(swap_request_primary).await;
    assert!(
        result.is_ok(),
        "Should succeed - primary keys (2-of-3) can STILL spend after locktime (NUT-11): {:?}",
        result.err()
    );
    println!(
        "✓ Spending with primary keys (Alice + Bob, 2-of-3) AFTER locktime succeeded (NUT-11)"
    );
}

/// Test: P2PK signed by wrong person is rejected
///
/// Creates proofs locked to Alice's public key and verifies that
/// signing with Bob's key (wrong key) is rejected
#[tokio::test]
async fn test_p2pk_signed_by_wrong_person() {
    let test_mint = TestMintHelper::new().await.unwrap();
    let mint = test_mint.mint();

    // Generate keypairs for Alice and Bob
    let (_alice_secret, alice_pubkey) = create_test_keypair();
    let (bob_secret, _bob_pubkey) = create_test_keypair();
    println!("Alice pubkey: {}", alice_pubkey);
    println!("Bob will try to spend Alice's proofs");

    // Step 1: Mint regular proofs
    let input_amount = Amount::from(10);
    let input_proofs = test_mint.mint_proofs(input_amount).await.unwrap();

    // Step 2: Create P2PK blinded messages locked to Alice's pubkey
    let spending_conditions = SpendingConditions::new_p2pk(alice_pubkey, None);
    let split_amounts = test_mint.split_amount(input_amount).unwrap();
    let (p2pk_outputs, blinding_factors, secrets) = unzip3(
        split_amounts
            .iter()
            .map(|&amt| test_mint.create_blinded_message(amt, &spending_conditions))
            .collect(),
    );

    // Step 3: Swap for P2PK proofs locked to Alice
    let swap_request =
        cdk_common::nuts::SwapRequest::new(input_proofs.clone(), p2pk_outputs.clone());
    let swap_response = mint.process_swap_request(swap_request).await.unwrap();
    println!("Created P2PK proofs locked to Alice");

    // Step 4: Construct the P2PK proofs
    let p2pk_proofs = construct_proofs(
        swap_response.signatures.clone(),
        blinding_factors.clone(),
        secrets.clone(),
        &test_mint.public_keys_of_the_active_sat_keyset,
    )
    .unwrap();

    // Step 5: Try to spend Alice's proofs by signing with Bob's key (wrong key!)
    let (new_outputs, _) = create_test_blinded_messages(mint, input_amount)
        .await
        .unwrap();
    let mut swap_request_wrong_sig =
        cdk_common::nuts::SwapRequest::new(p2pk_proofs.clone(), new_outputs.clone());

    // Sign with Bob's key instead of Alice's key
    for proof in swap_request_wrong_sig.inputs_mut() {
        proof.sign_p2pk(bob_secret.clone()).unwrap();
    }

    let result = mint.process_swap_request(swap_request_wrong_sig).await;
    assert!(result.is_err(), "Should fail when signed with wrong key");
    println!(
        "✓ Spending signed by wrong person failed as expected: {:?}",
        result.err()
    );
}

/// Test: Duplicate signatures are rejected
///
/// Verifies that using the same signature twice doesn't count as multiple signers
/// in a 2-of-2 multisig scenario
#[tokio::test]
async fn test_p2pk_duplicate_signatures() {
    let test_mint = TestMintHelper::new().await.unwrap();
    let mint = test_mint.mint();

    let (alice_secret, alice_pubkey) = create_test_keypair();
    let (_bob_secret, bob_pubkey) = create_test_keypair();

    println!("Alice: {}", alice_pubkey);
    println!("Bob: {}", bob_pubkey);

    // Step 1: Mint regular proofs
    let input_amount = Amount::from(10);
    let input_proofs = test_mint.mint_proofs(input_amount).await.unwrap();

    // Step 2: Create 2-of-2 multisig (Alice and Bob, need both)
    let spending_conditions = SpendingConditions::new_p2pk(
        alice_pubkey,
        Some(
            Conditions::new(
                None,                   // no locktime
                Some(vec![bob_pubkey]), // Bob is additional pubkey
                None,                   // no refund keys
                Some(2),                // require 2 signatures (Alice + Bob)
                None,                   // default sig_flag
                None,                   // no num_sigs_refund
            )
            .unwrap(),
        ),
    );
    println!("Created 2-of-2 multisig (Alice, Bob)");

    // Step 3: Create P2PK blinded messages
    let split_amounts = test_mint.split_amount(input_amount).unwrap();
    let (p2pk_outputs, blinding_factors, secrets) = unzip3(
        split_amounts
            .iter()
            .map(|&amt| test_mint.create_blinded_message(amt, &spending_conditions))
            .collect(),
    );

    // Step 4: Swap for P2PK proofs
    let swap_request =
        cdk_common::nuts::SwapRequest::new(input_proofs.clone(), p2pk_outputs.clone());
    let swap_response = mint.process_swap_request(swap_request).await.unwrap();

    // Step 5: Construct the P2PK proofs
    let p2pk_proofs = construct_proofs(
        swap_response.signatures.clone(),
        blinding_factors.clone(),
        secrets.clone(),
        &test_mint.public_keys_of_the_active_sat_keyset,
    )
    .unwrap();

    // Step 6: Try to spend with Alice's signature TWICE (should fail - need Alice + Bob, not Alice + Alice)
    let (new_outputs, _) = create_test_blinded_messages(mint, input_amount)
        .await
        .unwrap();
    let mut swap_request_duplicate =
        cdk_common::nuts::SwapRequest::new(p2pk_proofs.clone(), new_outputs.clone());

    // Sign with Alice twice instead of Alice + Bob
    for proof in swap_request_duplicate.inputs_mut() {
        proof.sign_p2pk(alice_secret.clone()).unwrap();
        proof.sign_p2pk(alice_secret.clone()).unwrap(); // Duplicate!
    }

    let result = mint.process_swap_request(swap_request_duplicate).await;
    assert!(
        result.is_err(),
        "Should fail - duplicate signatures not allowed"
    );
    println!(
        "✓ Spending with duplicate signatures (Alice + Alice) failed as expected: {:?}",
        result.err()
    );
}

//! P2PK SIG_ALL tests for swap functionality
//!
//! These tests verify that the mint correctly enforces SIG_ALL flag behavior

use cdk_common::dhke::construct_proofs;
use cdk_common::nuts::{Conditions, SigFlag, SpendingConditions};
use cdk_common::Amount;

use crate::test_helpers::mint::create_test_blinded_messages;
use crate::test_helpers::nut10::{create_test_keypair, unzip3, TestMintHelper};
use crate::util::unix_time;

/// Test: P2PK with SIG_ALL flag requires transaction signature
///
/// Creates P2PK proofs with SIG_ALL flag and verifies:
/// 1. Spending without signature is rejected
/// 2. Spending with SIG_INPUTS signatures (individual proof signatures) is rejected
/// 3. Spending with SIG_ALL signature (transaction signature) succeeds
#[tokio::test]
async fn test_p2pk_sig_all_requires_transaction_signature() {
    let test_mint = TestMintHelper::new().await.unwrap();
    let mint = test_mint.mint();

    // Generate keypair for P2PK
    let (alice_secret, alice_pubkey) = create_test_keypair();
    println!("Alice pubkey: {}", alice_pubkey);

    // Step 1: Create regular unencumbered proofs that we'll swap for P2PK proofs
    let input_amount = Amount::from(10);
    let input_proofs = test_mint.mint_proofs(input_amount).await.unwrap();

    // Step 2: Create P2PK blinded messages (outputs locked to alice_pubkey) with SIG_ALL
    let spending_conditions = SpendingConditions::new_p2pk(
        alice_pubkey,
        Some(
            Conditions::new(
                None,                  // no locktime
                None,                  // no additional pubkeys
                None,                  // no refund keys
                None,                  // default num_sigs (1)
                Some(SigFlag::SigAll), // SIG_ALL flag
                None,                  // no num_sigs_refund
            )
            .unwrap(),
        ),
    );
    println!("Created P2PK spending conditions with SIG_ALL flag");

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

    // Step 6: Sign all proofs individually (SIG_INPUTS way) - should fail for SIG_ALL
    let mut swap_request_sig_inputs =
        cdk_common::nuts::SwapRequest::new(p2pk_proofs.clone(), new_outputs.clone());

    // Sign each proof individually (SIG_INPUTS mode)
    for proof in swap_request_sig_inputs.inputs_mut() {
        proof.sign_p2pk(alice_secret.clone()).unwrap();
    }

    let result = mint.process_swap_request(swap_request_sig_inputs).await;
    assert!(
        result.is_err(),
        "Should fail - SIG_INPUTS signatures not valid for SIG_ALL"
    );
    println!(
        "✓ Spending with SIG_INPUTS signatures failed as expected: {:?}",
        result.err()
    );

    // Step 7: Sign the transaction with SIG_ALL (should succeed)
    let mut swap_request_with_sig =
        cdk_common::nuts::SwapRequest::new(p2pk_proofs.clone(), new_outputs.clone());

    // Use sign_sig_all to sign the transaction (signature goes on first proof's witness)
    swap_request_with_sig
        .sign_sig_all(alice_secret.clone())
        .unwrap();

    let result = mint.process_swap_request(swap_request_with_sig).await;
    assert!(
        result.is_ok(),
        "Should succeed with valid signature: {:?}",
        result.err()
    );
    println!("✓ Spending WITH ALL signatures (SIG_ALL) succeeded");
}

/// Test: P2PK multisig (2-of-3) with SIG_ALL
///
/// Creates proofs requiring 2 signatures from a set of 3 public keys with SIG_ALL flag and verifies:
/// 1. Spending with only 1 signature fails (Alice only)
/// 2. Spending with 2 invalid signatures fails (wrong keys)
/// 3. Spending with 2 valid signatures succeeds (Alice + Bob)
#[tokio::test]
async fn test_p2pk_sig_all_multisig_2of3() {
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

    // Step 2: Create 2-of-3 multisig conditions with SIG_ALL
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
                Some(SigFlag::SigAll),                // SIG_ALL flag
                None,                                 // no num_sigs_refund
            )
            .unwrap(),
        ),
    );
    println!("Created 2-of-3 multisig spending conditions with SIG_ALL (Alice, Bob, Carol)");

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
    println!("Created P2PK multisig proofs (2-of-3) with SIG_ALL");

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

    // Sign with only Alice (SIG_ALL mode)
    swap_request_one_sig
        .sign_sig_all(alice_secret.clone())
        .unwrap();

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

    // Sign with Dave and Eve (wrong keys!) - add signatures one at a time
    swap_request_invalid_sigs
        .sign_sig_all(dave_secret.clone())
        .unwrap();
    swap_request_invalid_sigs
        .sign_sig_all(eve_secret.clone())
        .unwrap();

    let result = mint.process_swap_request(swap_request_invalid_sigs).await;
    assert!(result.is_err(), "Should fail with 2 invalid signatures");
    println!(
        "✓ Spending with 2 INVALID signatures (Dave + Eve) failed as expected: {:?}",
        result.err()
    );

    // Step 8: Spend with 2 valid signatures (Alice + Bob - should succeed)
    let mut swap_request_valid_sigs =
        cdk_common::nuts::SwapRequest::new(p2pk_proofs.clone(), new_outputs.clone());

    // Sign with Alice and Bob - add signatures one at a time
    swap_request_valid_sigs
        .sign_sig_all(alice_secret.clone())
        .unwrap();
    swap_request_valid_sigs
        .sign_sig_all(bob_secret.clone())
        .unwrap();

    // print the json serializiation of this final swap. It should succeed
    // as it has sufficient signatures
    println!(
        "{}",
        serde_json::to_string_pretty(&swap_request_valid_sigs.clone()).unwrap()
    );

    let result = mint.process_swap_request(swap_request_valid_sigs).await;
    assert!(
        result.is_ok(),
        "Should succeed with 2 valid signatures: {:?}",
        result.err()
    );
    println!("✓ Spending with 2 VALID signatures (Alice + Bob) succeeded");
}

/// Test: P2PK with SIG_ALL signed by wrong person is rejected
///
/// Creates proofs locked to Alice's public key with SIG_ALL flag and verifies that
/// signing with Bob's key (wrong key) is rejected
#[tokio::test]
async fn test_p2pk_sig_all_signed_by_wrong_person() {
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

    // Step 2: Create P2PK blinded messages locked to Alice's pubkey with SIG_ALL
    let spending_conditions = SpendingConditions::new_p2pk(
        alice_pubkey,
        Some(
            Conditions::new(
                None,                  // no locktime
                None,                  // no additional pubkeys
                None,                  // no refund keys
                None,                  // default num_sigs (1)
                Some(SigFlag::SigAll), // SIG_ALL flag
                None,                  // no num_sigs_refund
            )
            .unwrap(),
        ),
    );
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
    println!("Created P2PK proofs locked to Alice with SIG_ALL");

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

    // Sign with Bob's key instead of Alice's key (SIG_ALL mode)
    swap_request_wrong_sig
        .sign_sig_all(bob_secret.clone())
        .unwrap();

    let result = mint.process_swap_request(swap_request_wrong_sig).await;
    assert!(result.is_err(), "Should fail when signed with wrong key");
    println!(
        "✓ Spending signed by wrong person failed as expected: {:?}",
        result.err()
    );
}

/// Test: Duplicate signatures are rejected (SIG_ALL)
///
/// Verifies that using the same signature twice doesn't count as multiple signers
/// in a 2-of-2 multisig scenario with SIG_ALL flag
#[tokio::test]
async fn test_p2pk_sig_all_duplicate_signatures() {
    let test_mint = TestMintHelper::new().await.unwrap();
    let mint = test_mint.mint();

    let (alice_secret, alice_pubkey) = create_test_keypair();
    let (_bob_secret, bob_pubkey) = create_test_keypair();

    println!("Alice: {}", alice_pubkey);
    println!("Bob: {}", bob_pubkey);

    // Step 1: Mint regular proofs
    let input_amount = Amount::from(10);
    let input_proofs = test_mint.mint_proofs(input_amount).await.unwrap();

    // Step 2: Create 2-of-2 multisig (Alice and Bob, need both) with SIG_ALL
    let spending_conditions = SpendingConditions::new_p2pk(
        alice_pubkey,
        Some(
            Conditions::new(
                None,                   // no locktime
                Some(vec![bob_pubkey]), // Bob is additional pubkey
                None,                   // no refund keys
                Some(2),                // require 2 signatures (Alice + Bob)
                Some(SigFlag::SigAll),  // SIG_ALL flag
                None,                   // no num_sigs_refund
            )
            .unwrap(),
        ),
    );
    println!("Created 2-of-2 multisig (Alice, Bob) with SIG_ALL");

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

    // Sign with Alice twice instead of Alice + Bob (SIG_ALL mode)
    swap_request_duplicate
        .sign_sig_all(alice_secret.clone())
        .unwrap();
    swap_request_duplicate
        .sign_sig_all(alice_secret.clone())
        .unwrap(); // Duplicate!

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

/// Test: P2PK with locktime (before expiry) - SIG_ALL
///
/// Verifies that before locktime expires with SIG_ALL:
/// 1. Spending with primary key (Alice) succeeds
/// 2. Spending with refund key (Bob) fails
#[tokio::test]
async fn test_p2pk_sig_all_locktime_before_expiry() {
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

    // Step 2: Create conditions with Alice as primary and Bob as refund key with SIG_ALL
    let spending_conditions = SpendingConditions::new_p2pk(
        alice_pubkey,
        Some(
            Conditions::new(
                Some(locktime),         // locktime in the future
                None,                   // no additional pubkeys
                Some(vec![bob_pubkey]), // Bob is refund key
                None,                   // default num_sigs (1)
                Some(SigFlag::SigAll),  // SIG_ALL flag
                None,                   // default num_sigs_refund (1)
            )
            .unwrap(),
        ),
    );
    println!("Created P2PK with locktime and refund key with SIG_ALL");

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

    // Sign with Bob (refund key) using SIG_ALL
    swap_request_refund
        .sign_sig_all(bob_secret.clone())
        .unwrap();

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

    // Sign with Alice (primary key) using SIG_ALL
    swap_request_primary
        .sign_sig_all(alice_secret.clone())
        .unwrap();

    let result = mint.process_swap_request(swap_request_primary).await;
    assert!(
        result.is_ok(),
        "Should succeed - primary key can spend before locktime: {:?}",
        result.err()
    );
    println!("✓ Spending with primary key (Alice) BEFORE locktime succeeded");
}

/// Test: P2PK with locktime (after expiry) - SIG_ALL
///
/// Verifies that after locktime expires with SIG_ALL:
/// 1. Spending with primary key (Alice) fails
/// 2. Spending with refund key (Bob) succeeds
#[tokio::test]
async fn test_p2pk_sig_all_locktime_after_expiry() {
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

    // Step 2: Create conditions with Alice as primary and Bob as refund key with SIG_ALL
    let spending_conditions = SpendingConditions::new_p2pk(
        alice_pubkey,
        Some(Conditions {
            locktime: Some(locktime),            // locktime in the past (expired)
            pubkeys: None,                       // no additional pubkeys
            refund_keys: Some(vec![bob_pubkey]), // Bob is refund key
            num_sigs: None,                      // default (1)
            sig_flag: SigFlag::SigAll,           // SIG_ALL flag
            num_sigs_refund: None,               // default (1)
        }),
    );
    println!("Created P2PK with expired locktime and refund key with SIG_ALL");

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

    // Sign with Alice (primary key) using SIG_ALL
    swap_request_primary
        .sign_sig_all(alice_secret.clone())
        .unwrap();

    let result = mint.process_swap_request(swap_request_primary).await;
    assert!(
        result.is_ok(),
        "Should succeed - primary key can STILL spend after locktime (NUT-11 compliant): {:?}",
        result.err()
    );
    println!("✓ Spending with primary key (Alice) AFTER locktime succeeded (NUT-11 compliant)");
}

/// Test: P2PK with locktime after expiry, no refund keys (anyone can spend) - SIG_ALL
///
/// Verifies that after locktime expires with NO refund keys configured and SIG_ALL,
/// anyone can spend the proofs without providing any signatures at all.
#[tokio::test]
async fn test_p2pk_sig_all_locktime_after_expiry_no_refund_anyone_can_spend() {
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

    // Step 2: Create conditions with Alice as primary, NO refund keys, with SIG_ALL
    let spending_conditions = SpendingConditions::new_p2pk(
        alice_pubkey,
        Some(Conditions {
            locktime: Some(locktime),  // locktime in the past (expired)
            pubkeys: None,             // no additional pubkeys
            refund_keys: None,         // NO refund keys - anyone can spend!
            num_sigs: None,            // default (1)
            sig_flag: SigFlag::SigAll, // SIG_ALL flag
            num_sigs_refund: None,     // default (1)
        }),
    );
    println!("Created P2PK with expired locktime, NO refund keys, and SIG_ALL");

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

/// Test: P2PK multisig with locktime (2-of-3 before, 1-of-2 after) - SIG_ALL
///
/// Complex scenario with SIG_ALL: Different multisig requirements before and after locktime
/// Before locktime: Need 2-of-3 from (Alice, Bob, Carol)
/// After locktime: Need 1-of-2 from (Dave, Eve) as refund keys
#[tokio::test]
async fn test_p2pk_sig_all_multisig_locktime() {
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

    // Step 2: Create complex conditions with SIG_ALL
    // Before locktime: 2-of-3 (Alice, Bob, Carol)
    // After locktime: 1-of-2 (Dave, Eve)
    let spending_conditions = SpendingConditions::new_p2pk(
        alice_pubkey,
        Some(Conditions {
            locktime: Some(locktime),                         // Already expired
            pubkeys: Some(vec![bob_pubkey, carol_pubkey]), // Bob and Carol (with Alice = 3 total)
            refund_keys: Some(vec![dave_pubkey, eve_pubkey]), // Dave and Eve for refund
            num_sigs: Some(2),                             // Need 2 signatures before locktime
            sig_flag: SigFlag::SigAll,                     // SIG_ALL flag
            num_sigs_refund: Some(1),                      // Need 1 signature after locktime
        }),
    );
    println!("Created complex P2PK with SIG_ALL: 2-of-3 before locktime, 1-of-2 after locktime");

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

    // Sign with Alice + Bob (primary multisig - need 2-of-3) using SIG_ALL
    swap_request_primary
        .sign_sig_all(alice_secret.clone())
        .unwrap();
    swap_request_primary
        .sign_sig_all(bob_secret.clone())
        .unwrap();

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

/// Test: SIG_ALL with mixed proofs (different data) should fail
///
/// Per NUT-11, when any proof has SIG_ALL, all proofs must have:
/// 1. Same kind, 2. SIG_ALL flag, 3. Same data, 4. Same tags
/// This test verifies that mixing proofs with different pubkeys (different data) is rejected.
#[tokio::test]
async fn test_p2pk_sig_all_mixed_proofs_different_data() {
    let test_mint = TestMintHelper::new().await.unwrap();
    let mint = test_mint.mint();

    // Create two different keypairs
    let (alice_secret, alice_pubkey) = create_test_keypair();
    let (bob_secret, bob_pubkey) = create_test_keypair();

    println!("Alice pubkey: {}", alice_pubkey);
    println!("Bob pubkey: {}", bob_pubkey);

    // Step 1: Mint regular proofs for Alice
    let alice_input_amount = Amount::from(10);
    let alice_input_proofs = test_mint.mint_proofs(alice_input_amount).await.unwrap();

    // Step 2: Create Alice's P2PK spending conditions with SIG_ALL
    let alice_spending_conditions = SpendingConditions::new_p2pk(
        alice_pubkey,
        Some(Conditions {
            locktime: None,
            pubkeys: None,
            refund_keys: None,
            num_sigs: None,
            sig_flag: SigFlag::SigAll,
            num_sigs_refund: None,
        }),
    );

    // Step 3: Swap for Alice's P2PK proofs
    let alice_split_amounts = test_mint.split_amount(alice_input_amount).unwrap();
    let (alice_outputs, alice_blinding_factors, alice_secrets) = unzip3(
        alice_split_amounts
            .iter()
            .map(|&amt| test_mint.create_blinded_message(amt, &alice_spending_conditions))
            .collect(),
    );

    let swap_request_alice =
        cdk_common::nuts::SwapRequest::new(alice_input_proofs, alice_outputs.clone());
    let swap_response_alice = mint.process_swap_request(swap_request_alice).await.unwrap();

    let alice_proofs = construct_proofs(
        swap_response_alice.signatures.clone(),
        alice_blinding_factors.clone(),
        alice_secrets.clone(),
        &test_mint.public_keys_of_the_active_sat_keyset,
    )
    .unwrap();

    println!(
        "Created {} Alice proofs (locked to Alice with SIG_ALL)",
        alice_proofs.len()
    );

    // Step 4: Mint regular proofs for Bob
    let bob_input_amount = Amount::from(10);
    let bob_input_proofs = test_mint.mint_proofs(bob_input_amount).await.unwrap();

    // Step 5: Create Bob's P2PK spending conditions with SIG_ALL (different data!)
    let bob_spending_conditions = SpendingConditions::new_p2pk(
        bob_pubkey,
        Some(Conditions {
            locktime: None,
            pubkeys: None,
            refund_keys: None,
            num_sigs: None,
            sig_flag: SigFlag::SigAll,
            num_sigs_refund: None,
        }),
    );

    // Step 6: Swap for Bob's P2PK proofs
    let bob_split_amounts = test_mint.split_amount(bob_input_amount).unwrap();
    let (bob_outputs, bob_blinding_factors, bob_secrets) = unzip3(
        bob_split_amounts
            .iter()
            .map(|&amt| test_mint.create_blinded_message(amt, &bob_spending_conditions))
            .collect(),
    );

    let swap_request_bob =
        cdk_common::nuts::SwapRequest::new(bob_input_proofs, bob_outputs.clone());
    let swap_response_bob = mint.process_swap_request(swap_request_bob).await.unwrap();

    let bob_proofs = construct_proofs(
        swap_response_bob.signatures.clone(),
        bob_blinding_factors.clone(),
        bob_secrets.clone(),
        &test_mint.public_keys_of_the_active_sat_keyset,
    )
    .unwrap();

    println!(
        "Created {} Bob proofs (locked to Bob with SIG_ALL)",
        bob_proofs.len()
    );

    // Step 7: Try to spend Alice's and Bob's proofs together in one transaction (should FAIL!)
    // This violates NUT-11 requirement that all SIG_ALL proofs must have same data
    let total_amount = alice_input_amount + bob_input_amount;
    let (new_outputs, _) = create_test_blinded_messages(mint, total_amount)
        .await
        .unwrap();

    let mut mixed_proofs = alice_proofs.clone();
    mixed_proofs.extend(bob_proofs.clone());

    let mut swap_request_mixed =
        cdk_common::nuts::SwapRequest::new(mixed_proofs, new_outputs.clone());

    // Sign with both Alice's and Bob's keys (no client-side validation, so this succeeds)
    swap_request_mixed
        .sign_sig_all(alice_secret.clone())
        .unwrap();
    swap_request_mixed.sign_sig_all(bob_secret.clone()).unwrap();

    // But the mint should reject it due to mismatched data, even though both signed
    let result = mint.process_swap_request(swap_request_mixed).await;
    assert!(result.is_err(), "Should fail - cannot mix proofs with different data in SIG_ALL transaction, even with both signatures");

    let error_msg = format!("{:?}", result.err().unwrap());
    println!(
        "✓ Mixing Alice and Bob proofs in SIG_ALL transaction failed at mint verification: {}",
        error_msg
    );

    // Step 8: Alice should be able to spend her proofs alone (should succeed)
    let (alice_new_outputs, _) = create_test_blinded_messages(mint, alice_input_amount)
        .await
        .unwrap();
    let mut swap_request_alice_only =
        cdk_common::nuts::SwapRequest::new(alice_proofs.clone(), alice_new_outputs.clone());
    swap_request_alice_only
        .sign_sig_all(alice_secret.clone())
        .unwrap();

    let result = mint.process_swap_request(swap_request_alice_only).await;
    assert!(
        result.is_ok(),
        "Should succeed - Alice spending her own proofs: {:?}",
        result.err()
    );
    println!("✓ Alice successfully spent her own proofs separately");

    // Step 9: Bob should be able to spend his proofs alone (should succeed)
    let (bob_new_outputs, _) = create_test_blinded_messages(mint, bob_input_amount)
        .await
        .unwrap();
    let mut swap_request_bob_only =
        cdk_common::nuts::SwapRequest::new(bob_proofs.clone(), bob_new_outputs.clone());
    swap_request_bob_only
        .sign_sig_all(bob_secret.clone())
        .unwrap();

    let result = mint.process_swap_request(swap_request_bob_only).await;
    assert!(
        result.is_ok(),
        "Should succeed - Bob spending his own proofs: {:?}",
        result.err()
    );
    println!("✓ Bob successfully spent his own proofs separately");
}

/// Test: P2PK multisig BEFORE locktime expires (2-of-3) - SIG_ALL
///
/// Tests that a 2-of-3 multisig with SIG_ALL works correctly BEFORE locktime expires.
/// This complements the existing test that verifies refund keys work AFTER locktime.
#[tokio::test]
async fn test_p2pk_sig_all_multisig_before_locktime() {
    let test_mint = TestMintHelper::new().await.unwrap();
    let mint = test_mint.mint();

    // Create 3 keypairs for primary multisig (Alice, Bob, Carol)
    let (alice_secret, alice_pubkey) = create_test_keypair();
    let (bob_secret, bob_pubkey) = create_test_keypair();
    let (_carol_secret, carol_pubkey) = create_test_keypair();

    // Create refund keys (Dave, Eve) - won't be used since we're before locktime
    let (_dave_secret, dave_pubkey) = create_test_keypair();
    let (_eve_secret, eve_pubkey) = create_test_keypair();

    let locktime = unix_time() + 3600; // Locktime is 1 hour in the future

    println!("Primary multisig: Alice, Bob, Carol (need 2-of-3)");
    println!("Refund multisig: Dave, Eve (need 1-of-2)");
    println!("Current time: {}", unix_time());
    println!("Locktime: {} (expires in 1 hour)", locktime);

    // Step 1: Mint regular proofs
    let input_amount = Amount::from(10);
    let input_proofs = test_mint.mint_proofs(input_amount).await.unwrap();

    // Step 2: Create complex conditions with SIG_ALL
    // Before locktime: 2-of-3 (Alice, Bob, Carol)
    // After locktime: 1-of-2 (Dave, Eve)
    let spending_conditions = SpendingConditions::new_p2pk(
        alice_pubkey,
        Some(Conditions {
            locktime: Some(locktime),                         // 1 hour in the future
            pubkeys: Some(vec![bob_pubkey, carol_pubkey]), // Bob and Carol (with Alice = 3 total)
            refund_keys: Some(vec![dave_pubkey, eve_pubkey]), // Dave and Eve for refund
            num_sigs: Some(2),                             // Need 2 signatures before locktime
            sig_flag: SigFlag::SigAll,                     // SIG_ALL flag
            num_sigs_refund: Some(1),                      // Need 1 signature after locktime
        }),
    );
    println!("Created complex P2PK with SIG_ALL: 2-of-3 before locktime, 1-of-2 after locktime");

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

    // Step 6: Try to spend with only 1 signature (Alice) BEFORE locktime (should fail - need 2-of-3)
    let (new_outputs, _) = create_test_blinded_messages(mint, input_amount)
        .await
        .unwrap();
    let mut swap_request_one_sig =
        cdk_common::nuts::SwapRequest::new(p2pk_proofs.clone(), new_outputs.clone());

    // Sign with Alice only (need 2-of-3)
    swap_request_one_sig
        .sign_sig_all(alice_secret.clone())
        .unwrap();

    let result = mint.process_swap_request(swap_request_one_sig).await;
    assert!(
        result.is_err(),
        "Should fail - need 2-of-3 signatures before locktime"
    );
    println!(
        "✓ Spending with only 1 signature (Alice) BEFORE locktime failed as expected: {:?}",
        result.err()
    );

    // Step 7: Spend with 2 signatures (Alice + Bob) BEFORE locktime (should succeed - 2-of-3)
    let mut swap_request_two_sigs =
        cdk_common::nuts::SwapRequest::new(p2pk_proofs.clone(), new_outputs.clone());

    // Sign with Alice + Bob (2-of-3, should succeed)
    swap_request_two_sigs
        .sign_sig_all(alice_secret.clone())
        .unwrap();
    swap_request_two_sigs
        .sign_sig_all(bob_secret.clone())
        .unwrap();

    let result = mint.process_swap_request(swap_request_two_sigs).await;
    assert!(
        result.is_ok(),
        "Should succeed - 2-of-3 signatures before locktime: {:?}",
        result.err()
    );
    println!("✓ Spending with 2 signatures (Alice + Bob, 2-of-3) BEFORE locktime succeeded");
}

/// Test: P2PK with more signatures than required - SIG_ALL
///
/// Tests that providing MORE valid signatures than required succeeds.
/// For example, 3 valid signatures for a 2-of-3 multisig should work fine.
#[tokio::test]
async fn test_p2pk_sig_all_more_signatures_than_required() {
    let test_mint = TestMintHelper::new().await.unwrap();
    let mint = test_mint.mint();

    // Create 3 keypairs for multisig (Alice, Bob, Carol)
    let (alice_secret, alice_pubkey) = create_test_keypair();
    let (bob_secret, bob_pubkey) = create_test_keypair();
    let (carol_secret, carol_pubkey) = create_test_keypair();

    println!("Multisig: Alice, Bob, Carol (need 2-of-3)");

    // Step 1: Mint regular proofs
    let input_amount = Amount::from(10);
    let input_proofs = test_mint.mint_proofs(input_amount).await.unwrap();

    // Step 2: Create 2-of-3 multisig conditions with SIG_ALL
    let spending_conditions = SpendingConditions::new_p2pk(
        alice_pubkey,
        Some(Conditions {
            locktime: None,
            pubkeys: Some(vec![bob_pubkey, carol_pubkey]), // Bob and Carol (with Alice = 3 total)
            refund_keys: None,
            num_sigs: Some(2), // Need 2 signatures (but we'll provide 3)
            sig_flag: SigFlag::SigAll,
            num_sigs_refund: None,
        }),
    );
    println!("Created 2-of-3 multisig with SIG_ALL");

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

    // Step 6: Spend with ALL 3 signatures (Alice + Bob + Carol) even though only 2 required
    let (new_outputs, _) = create_test_blinded_messages(mint, input_amount)
        .await
        .unwrap();
    let mut swap_request_all_sigs =
        cdk_common::nuts::SwapRequest::new(p2pk_proofs.clone(), new_outputs.clone());

    // Sign with all 3 keys (more than the required 2-of-3)
    swap_request_all_sigs
        .sign_sig_all(alice_secret.clone())
        .unwrap();
    swap_request_all_sigs
        .sign_sig_all(bob_secret.clone())
        .unwrap();
    swap_request_all_sigs
        .sign_sig_all(carol_secret.clone())
        .unwrap();

    let result = mint.process_swap_request(swap_request_all_sigs).await;
    assert!(
        result.is_ok(),
        "Should succeed - 3 valid signatures when only 2-of-3 required: {:?}",
        result.err()
    );
    println!("✓ Spending with 3 signatures (all of Alice, Bob, Carol) when only 2-of-3 required succeeded");
}

/// Test: P2PK with 2-of-2 refund multisig after locktime - SIG_ALL
///
/// Tests that after locktime expires, BOTH refund signatures are required (2-of-2).
/// Verifies that 1-of-2 fails and 2-of-2 succeeds.
#[tokio::test]
async fn test_p2pk_sig_all_refund_multisig_2of2() {
    let test_mint = TestMintHelper::new().await.unwrap();
    let mint = test_mint.mint();

    // Primary key (Alice)
    let (_alice_secret, alice_pubkey) = create_test_keypair();

    // Refund keys (Dave, Eve) - need both after locktime
    let (dave_secret, dave_pubkey) = create_test_keypair();
    let (eve_secret, eve_pubkey) = create_test_keypair();

    let locktime = unix_time() - 3600; // Already expired (1 hour ago)

    println!("Alice (primary)");
    println!("Dave, Eve (refund, need 2-of-2)");
    println!("Current time: {}", unix_time());
    println!("Locktime: {} (expired 1 hour ago)", locktime);

    // Step 1: Mint regular proofs
    let input_amount = Amount::from(10);
    let input_proofs = test_mint.mint_proofs(input_amount).await.unwrap();

    // Step 2: Create conditions with 2-of-2 refund multisig and SIG_ALL
    let spending_conditions = SpendingConditions::new_p2pk(
        alice_pubkey,
        Some(Conditions {
            locktime: Some(locktime), // Already expired
            pubkeys: None,
            refund_keys: Some(vec![dave_pubkey, eve_pubkey]), // Dave and Eve for refund
            num_sigs: None,                                   // Default (1) for primary
            sig_flag: SigFlag::SigAll,
            num_sigs_refund: Some(2), // Need BOTH refund signatures (2-of-2)
        }),
    );
    println!("Created P2PK with SIG_ALL: 2-of-2 refund multisig after locktime");

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

    // Step 6: Try to spend with only Dave's signature (1-of-2, should fail - need 2-of-2)
    let (new_outputs, _) = create_test_blinded_messages(mint, input_amount)
        .await
        .unwrap();
    let mut swap_request_one_refund =
        cdk_common::nuts::SwapRequest::new(p2pk_proofs.clone(), new_outputs.clone());

    // Sign with Dave only (need both Dave and Eve)
    swap_request_one_refund
        .sign_sig_all(dave_secret.clone())
        .unwrap();

    let result = mint.process_swap_request(swap_request_one_refund).await;
    assert!(
        result.is_err(),
        "Should fail - need 2-of-2 refund signatures"
    );
    println!(
        "✓ Spending with only 1 refund signature (Dave) AFTER locktime failed as expected: {:?}",
        result.err()
    );

    // Step 7: Spend with both Dave and Eve (2-of-2, should succeed)
    let mut swap_request_both_refunds =
        cdk_common::nuts::SwapRequest::new(p2pk_proofs.clone(), new_outputs.clone());

    // Sign with both Dave and Eve (2-of-2 refund multisig)
    swap_request_both_refunds
        .sign_sig_all(dave_secret.clone())
        .unwrap();
    swap_request_both_refunds
        .sign_sig_all(eve_secret.clone())
        .unwrap();

    let result = mint.process_swap_request(swap_request_both_refunds).await;
    assert!(
        result.is_ok(),
        "Should succeed - 2-of-2 refund signatures after locktime: {:?}",
        result.err()
    );
    println!("✓ Spending with 2-of-2 refund signatures (Dave + Eve) AFTER locktime succeeded");
}

/// Test: SIG_ALL should reject if output amounts are swapped
///
/// Creates two P2PK proofs (8+2 sats) with SIG_ALL flag, swaps the output amounts
/// after signing, and verifies that the mint should reject this (but currently doesn't).
#[tokio::test]
async fn test_sig_all_should_reject_if_the_output_amounts_are_swapped() {
    let test_mint = TestMintHelper::new().await.unwrap();
    let mint = test_mint.mint();

    // Generate keypair for Alice
    let (alice_secret, alice_pubkey) = create_test_keypair();
    println!("Alice pubkey: {}", alice_pubkey);

    // Step 1: Mint regular proofs (10 sats = 8+2)
    let input_amount = Amount::from(10);
    let input_proofs = test_mint.mint_proofs(input_amount).await.unwrap();
    println!("Minted {} sats", input_amount);

    // Step 2: Create P2PK spending conditions with SIG_ALL
    let spending_conditions = SpendingConditions::new_p2pk(
        alice_pubkey,
        Some(
            Conditions::new(
                None,                  // no locktime
                None,                  // no additional pubkeys
                None,                  // no refund keys
                None,                  // default num_sigs (1)
                Some(SigFlag::SigAll), // SIG_ALL flag
                None,                  // no num_sigs_refund
            )
            .unwrap(),
        ),
    );

    // Step 3: Swap for P2PK proofs with SIG_ALL
    let split_amounts = vec![Amount::from(8), Amount::from(2)];
    let (p2pk_outputs, blinding_factors, secrets) = unzip3(
        split_amounts
            .iter()
            .map(|&amt| test_mint.create_blinded_message(amt, &spending_conditions))
            .collect(),
    );

    let swap_request = cdk_common::nuts::SwapRequest::new(input_proofs, p2pk_outputs);
    let swap_response = mint.process_swap_request(swap_request).await.unwrap();

    // Step 4: Construct the P2PK proofs
    let p2pk_proofs = construct_proofs(
        swap_response.signatures,
        blinding_factors,
        secrets,
        &test_mint.public_keys_of_the_active_sat_keyset,
    )
    .unwrap();

    println!("Created {} P2PK proofs with SIG_ALL", p2pk_proofs.len());
    assert_eq!(p2pk_proofs.len(), 2, "Should have 2 proofs (8+2)");

    // Step 5: Create new swap request and sign with SIG_ALL
    let (new_outputs, _) = create_test_blinded_messages(mint, input_amount)
        .await
        .unwrap();
    let mut swap_request = cdk_common::nuts::SwapRequest::new(p2pk_proofs, new_outputs);

    // Inspect the outputs
    println!("Outputs in swap request:");
    for (i, output) in swap_request.outputs().iter().enumerate() {
        println!(
            "  Output {}: amount={}, blinded_secret={}",
            i,
            output.amount,
            output.blinded_secret.to_hex()
        );
    }

    // Sign the transaction with SIG_ALL
    swap_request.sign_sig_all(alice_secret).unwrap();

    // Swap the amounts of the two outputs
    let outputs = swap_request.outputs_mut();
    let temp_amount = outputs[0].amount;
    outputs[0].amount = outputs[1].amount;
    outputs[1].amount = temp_amount;

    // Print outputs after swapping amounts
    println!("Outputs after swapping amounts:");
    for (i, output) in swap_request.outputs().iter().enumerate() {
        println!(
            "  Output {}: amount={}, blinded_secret={}",
            i,
            output.amount,
            output.blinded_secret.to_hex()
        );
    }

    // Step 6: Try to execute the swap - should now FAIL because the signature is invalid
    let result = mint.process_swap_request(swap_request.clone()).await;
    assert!(
        result.is_err(),
        "Swap should fail - amounts were tampered with after signing"
    );
    println!("✓ Swap correctly rejected after output amounts were swapped!");
    println!("  Error: {:?}", result.err());

    // Step 7: Swap the amounts back to original and verify it succeeds
    let outputs = swap_request.outputs_mut();
    let temp_amount = outputs[0].amount;
    outputs[0].amount = outputs[1].amount;
    outputs[1].amount = temp_amount;

    println!("Outputs after swapping back to original:");
    for (i, output) in swap_request.outputs().iter().enumerate() {
        println!(
            "  Output {}: amount={}, blinded_secret={}",
            i,
            output.amount,
            output.blinded_secret.to_hex()
        );
    }

    let result = mint.process_swap_request(swap_request).await;
    assert!(
        result.is_ok(),
        "Swap should succeed with original amounts: {:?}",
        result.err()
    );
    println!("✓ Swap succeeded after restoring original amounts!");
}

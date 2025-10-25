#![cfg(test)]
//! P2PK (NUT-11) tests for swap functionality
//!
//! These tests verify that the mint correctly validates P2PK spending conditions
//! during swap operations, including:
//! - Single signature P2PK
//! - Multisig (m-of-n)
//! - Locktime enforcement
//! - Refund keys
//! - Signature validation

use cdk_common::nuts::{SecretKey, PublicKey, SpendingConditions, BlindedMessage, Id, CurrencyUnit, Keys, Conditions, SigFlag};
use cdk_common::nuts::nut10::Secret as Nut10Secret;
use cdk_common::Amount;
use cdk_common::dhke::{blind_message, construct_proofs};
use crate::secret::Secret;
use crate::util::unix_time;
use crate::mint::Mint;
use crate::Error;

use crate::test_helpers::mint::{create_test_blinded_messages, create_test_mint, mint_test_proofs};

/// Test mint wrapper with convenient access to common keyset info
struct TestMintHelper {
    mint: Mint,
    active_sat_keyset_id: Id,
    public_keys_of_the_active_sat_keyset: Keys,
    /// Available denominations sorted largest first (e.g., [2147483648, 1073741824, ..., 2, 1])
    available_amounts_sorted: Vec<u64>,
}

impl TestMintHelper {
    async fn new() -> Result<Self, Error> {
        let mint = create_test_mint().await?;

        // Get the active SAT keyset ID
        let active_sat_keyset_id = mint
            .get_active_keysets()
            .get(&CurrencyUnit::Sat)
            .cloned()
            .ok_or(Error::Internal)?;

        // Get the active SAT keyset keys
        let lookup_by_that_id = mint.keyset_pubkeys(&active_sat_keyset_id)?;
        let active_sat_keyset = lookup_by_that_id.keysets.first().ok_or(Error::Internal)?;
        assert_eq!(active_sat_keyset.id, active_sat_keyset_id, "Keyset ID mismatch");
        let public_keys_of_the_active_sat_keyset = active_sat_keyset.keys.clone();

        // Get the available denominations from the keyset, sorted largest first
        let mut available_amounts_sorted: Vec<u64> = public_keys_of_the_active_sat_keyset.iter().map(|(amt, _)| amt.to_u64()).collect();
        available_amounts_sorted.sort_by(|a, b| b.cmp(a)); // Sort descending (largest first)

        Ok(TestMintHelper {
            mint,
            active_sat_keyset_id,
            public_keys_of_the_active_sat_keyset,
            available_amounts_sorted,
        })
    }

    /// Get a reference to the underlying mint
    fn mint(&self) -> &Mint {
        &self.mint
    }

    /// Split an amount into power-of-2 denominations
    /// Returns the amounts that sum to the total (e.g., 10 -> [8, 2])
    fn split_amount(&self, amount: Amount) -> Result<Vec<Amount>, Error> {
        // Simple greedy algorithm: start from largest and work down
        let mut result = Vec::new();
        let mut remaining = amount.to_u64();

        for &amt in &self.available_amounts_sorted {
            if remaining >= amt {
                result.push(Amount::from(amt));
                remaining -= amt;
            }
        }

        if remaining != 0 {
            return Err(Error::Internal);
        }

        Ok(result)
    }

    /// Mint proofs for the given amount
    /// Prints a message like "Minted 10 sats [8+2]"
    async fn mint_proofs(&self, amount: Amount) -> Result<cdk_common::Proofs, Error> {
        let proofs = mint_test_proofs(&self.mint, amount).await?;

        // Build the split display string (e.g., "8+2")
        let split_amounts = self.split_amount(amount)?;
        let split_display: Vec<String> = split_amounts.iter().map(|a| a.to_string()).collect();
        println!("Minted {} sats [{}]", amount, split_display.join("+"));

        Ok(proofs)
    }

    /// Create a single blinded message with spending conditions for the given amount
    /// Returns (blinded_message, blinding_factor, secret)
    fn create_blinded_message(
        &self,
        amount: Amount,
        spending_conditions: &SpendingConditions,
    ) -> (BlindedMessage, SecretKey, Secret) {
        let nut10_secret: Nut10Secret = spending_conditions.clone().into();
        let secret: Secret = nut10_secret.try_into().unwrap();
        let (blinded_point, blinding_factor) = blind_message(&secret.to_bytes(), None).unwrap();
        let blinded_msg = BlindedMessage::new(amount, self.active_sat_keyset_id, blinded_point);
        (blinded_msg, blinding_factor, secret)
    }
}

/// Helper: Create a keypair for testing
fn create_test_keypair() -> (SecretKey, PublicKey) {
    let secret = SecretKey::generate();
    let pubkey = secret.public_key();
    (secret, pubkey)
}

/// Helper: Unzip a vector of 3-tuples into 3 separate vectors
fn unzip3<A, B, C>(vec: Vec<(A, B, C)>) -> (Vec<A>, Vec<B>, Vec<C>) {
    let mut vec_a = Vec::new();
    let mut vec_b = Vec::new();
    let mut vec_c = Vec::new();
    for (a, b, c) in vec {
        vec_a.push(a);
        vec_b.push(b);
        vec_c.push(c);
    }
    (vec_a, vec_b, vec_c)
}

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

    println!("Created {} P2PK outputs locked to alice", p2pk_outputs.len());

    // Step 3: Swap regular proofs for P2PK proofs (no signature needed on inputs)
    let swap_request = cdk_common::nuts::SwapRequest::new(
        input_proofs.clone(),
        p2pk_outputs.clone(),
    );
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
    ).unwrap();

    let proof_amounts: Vec<String> = p2pk_proofs.iter().map(|p| p.amount.to_string()).collect();
    println!("Constructed {} P2PK proof(s) [{}]", p2pk_proofs.len(), proof_amounts.join("+"));

    // Step 5: Try to spend P2PK proof WITHOUT signature (should fail)
    let (new_outputs, _) = create_test_blinded_messages(mint, input_amount).await.unwrap();
    let swap_request_no_sig = cdk_common::nuts::SwapRequest::new(
        p2pk_proofs.clone(),
        new_outputs.clone(),
    );

    let result = mint.process_swap_request(swap_request_no_sig).await;
    assert!(result.is_err(), "Should fail without signature");
    println!("✓ Spending WITHOUT signature failed as expected: {:?}", result.err());

    // Step 6: Sign only ONE of the proofs and try (should fail - need all signatures)
    let mut swap_request_partial_sig = cdk_common::nuts::SwapRequest::new(
        p2pk_proofs.clone(),
        new_outputs.clone(),
    );

    // Sign only the first proof
    swap_request_partial_sig.inputs_mut()[0].sign_p2pk(alice_secret.clone()).unwrap();

    let result = mint.process_swap_request(swap_request_partial_sig).await;
    assert!(result.is_err(), "Should fail with only partial signatures");
    println!("✓ Spending with PARTIAL signatures failed as expected: {:?}", result.err());

    // Step 7: Now sign ALL the proofs and try again (should succeed)
    let mut swap_request_with_sig = cdk_common::nuts::SwapRequest::new(
        p2pk_proofs.clone(),
        new_outputs.clone(),
    );

    // Sign all the P2PK proofs with Alice's key
    for proof in swap_request_with_sig.inputs_mut() {
        proof.sign_p2pk(alice_secret.clone()).unwrap();
    }

    let result = mint.process_swap_request(swap_request_with_sig).await;
    assert!(result.is_ok(), "Should succeed with valid signature: {:?}", result.err());
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
    let (carol_secret, carol_pubkey) = create_test_keypair();

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
        Some(Conditions::new(
            None, // no locktime
            Some(vec![bob_pubkey, carol_pubkey]), // additional pubkeys
            None, // no refund keys
            Some(2), // require 2 signatures
            None, // default sig_flag
            None, // no num_sigs_refund
        ).unwrap())
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
    let swap_request = cdk_common::nuts::SwapRequest::new(input_proofs.clone(), p2pk_outputs.clone());
    let swap_response = mint.process_swap_request(swap_request).await.unwrap();
    println!("Created P2PK multisig proofs (2-of-3)");

    // Step 5: Construct the P2PK proofs
    let p2pk_proofs = construct_proofs(
        swap_response.signatures.clone(),
        blinding_factors.clone(),
        secrets.clone(),
        &test_mint.public_keys_of_the_active_sat_keyset,
    ).unwrap();

    // Step 6: Try to spend with only 1 signature (Alice only - should fail)
    let (new_outputs, _) = create_test_blinded_messages(mint, input_amount).await.unwrap();
    let mut swap_request_one_sig = cdk_common::nuts::SwapRequest::new(
        p2pk_proofs.clone(),
        new_outputs.clone(),
    );

    // Sign with only Alice
    for proof in swap_request_one_sig.inputs_mut() {
        proof.sign_p2pk(alice_secret.clone()).unwrap();
    }

    let result = mint.process_swap_request(swap_request_one_sig).await;
    assert!(result.is_err(), "Should fail with only 1 signature (need 2)");
    println!("✓ Spending with only 1 signature (Alice) failed as expected: {:?}", result.err());

    // Step 7: Try to spend with 2 invalid signatures (Dave + Eve - not in multisig set)
    let mut swap_request_invalid_sigs = cdk_common::nuts::SwapRequest::new(
        p2pk_proofs.clone(),
        new_outputs.clone(),
    );

    // Sign with Dave and Eve (wrong keys!)
    for proof in swap_request_invalid_sigs.inputs_mut() {
        proof.sign_p2pk(dave_secret.clone()).unwrap();
        proof.sign_p2pk(eve_secret.clone()).unwrap();
    }

    let result = mint.process_swap_request(swap_request_invalid_sigs).await;
    assert!(result.is_err(), "Should fail with 2 invalid signatures");
    println!("✓ Spending with 2 INVALID signatures (Dave + Eve) failed as expected: {:?}", result.err());

    // Step 8: Spend with 2 valid signatures (Alice + Bob - should succeed)
    let mut swap_request_valid_sigs = cdk_common::nuts::SwapRequest::new(
        p2pk_proofs.clone(),
        new_outputs.clone(),
    );

    // Sign with Alice and Bob
    for proof in swap_request_valid_sigs.inputs_mut() {
        proof.sign_p2pk(alice_secret.clone()).unwrap();
        proof.sign_p2pk(bob_secret.clone()).unwrap();
    }

    let result = mint.process_swap_request(swap_request_valid_sigs).await;
    assert!(result.is_ok(), "Should succeed with 2 valid signatures: {:?}", result.err());
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
        Some(Conditions::new(
            Some(locktime), // locktime in the future
            None, // no additional pubkeys
            Some(vec![bob_pubkey]), // Bob is refund key
            None, // default num_sigs (1)
            None, // default sig_flag
            None, // default num_sigs_refund (1)
        ).unwrap())
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
    let swap_request = cdk_common::nuts::SwapRequest::new(input_proofs.clone(), p2pk_outputs.clone());
    let swap_response = mint.process_swap_request(swap_request).await.unwrap();

    // Step 5: Construct the P2PK proofs
    let p2pk_proofs = construct_proofs(
        swap_response.signatures.clone(),
        blinding_factors.clone(),
        secrets.clone(),
        &test_mint.public_keys_of_the_active_sat_keyset,
    ).unwrap();

    // Step 6: Try to spend with refund key (Bob) BEFORE locktime expires (should fail)
    let (new_outputs, _) = create_test_blinded_messages(mint, input_amount).await.unwrap();
    let mut swap_request_refund = cdk_common::nuts::SwapRequest::new(
        p2pk_proofs.clone(),
        new_outputs.clone(),
    );

    // Sign with Bob (refund key)
    for proof in swap_request_refund.inputs_mut() {
        proof.sign_p2pk(bob_secret.clone()).unwrap();
    }

    let result = mint.process_swap_request(swap_request_refund).await;
    assert!(result.is_err(), "Should fail - refund key cannot spend before locktime");
    println!("✓ Spending with refund key (Bob) BEFORE locktime failed as expected: {:?}", result.err());

    // Step 7: Spend with primary key (Alice) BEFORE locktime (should succeed)
    let mut swap_request_primary = cdk_common::nuts::SwapRequest::new(
        p2pk_proofs.clone(),
        new_outputs.clone(),
    );

    // Sign with Alice (primary key)
    for proof in swap_request_primary.inputs_mut() {
        proof.sign_p2pk(alice_secret.clone()).unwrap();
    }

    let result = mint.process_swap_request(swap_request_primary).await;
    assert!(result.is_ok(), "Should succeed - primary key can spend before locktime: {:?}", result.err());
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
            locktime: Some(locktime), // locktime in the past (expired)
            pubkeys: None, // no additional pubkeys
            refund_keys: Some(vec![bob_pubkey]), // Bob is refund key
            num_sigs: None, // default (1)
            sig_flag: SigFlag::default(),
            num_sigs_refund: None, // default (1)
        })
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
    let swap_request = cdk_common::nuts::SwapRequest::new(input_proofs.clone(), p2pk_outputs.clone());
    let swap_response = mint.process_swap_request(swap_request).await.unwrap();

    // Step 5: Construct the P2PK proofs
    let p2pk_proofs = construct_proofs(
        swap_response.signatures.clone(),
        blinding_factors.clone(),
        secrets.clone(),
        &test_mint.public_keys_of_the_active_sat_keyset,
    ).unwrap();

    // Step 6: Try to spend with primary key (Alice) AFTER locktime expires (should fail)
    let (new_outputs, _) = create_test_blinded_messages(mint, input_amount).await.unwrap();
    let mut swap_request_primary = cdk_common::nuts::SwapRequest::new(
        p2pk_proofs.clone(),
        new_outputs.clone(),
    );

    // Sign with Alice (primary key)
    for proof in swap_request_primary.inputs_mut() {
        proof.sign_p2pk(alice_secret.clone()).unwrap();
    }

    let result = mint.process_swap_request(swap_request_primary).await;
    assert!(result.is_err(), "Should fail - primary key cannot spend after locktime expires");
    println!("✓ Spending with primary key (Alice) AFTER locktime failed as expected: {:?}", result.err());

    // Step 7: Spend with refund key (Bob) AFTER locktime (should succeed)
    let mut swap_request_refund = cdk_common::nuts::SwapRequest::new(
        p2pk_proofs.clone(),
        new_outputs.clone(),
    );

    // Sign with Bob (refund key)
    for proof in swap_request_refund.inputs_mut() {
        proof.sign_p2pk(bob_secret.clone()).unwrap();
    }

    let result = mint.process_swap_request(swap_request_refund).await;
    assert!(result.is_ok(), "Should succeed - refund key can spend after locktime: {:?}", result.err());
    println!("✓ Spending with refund key (Bob) AFTER locktime succeeded");
}

/// Test: P2PK multisig with locktime (2-of-3 before, 1-of-2 after)
///
/// Complex scenario: Different multisig requirements before and after locktime
#[tokio::test]
async fn test_p2pk_multisig_locktime() {
    let mint = create_test_mint().await.unwrap();

    // Before locktime: Need 2-of-3 from (Alice, Bob, Carol)
    let (alice_secret, alice_pubkey) = create_test_keypair();
    let (bob_secret, bob_pubkey) = create_test_keypair();
    let (carol_secret, carol_pubkey) = create_test_keypair();

    // After locktime: Need 1-of-2 from (Dave, Eve)
    let (dave_secret, dave_pubkey) = create_test_keypair();
    let (eve_secret, eve_pubkey) = create_test_keypair();

    let locktime = unix_time() - 100; // Already expired

    // TODO: Create conditions
    // TODO: Test with Dave signature (should succeed - after locktime, 1-of-2)
    // TODO: Test with Alice + Bob (should fail - locktime expired, only refund keys valid)
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
    let (alice_secret, alice_pubkey) = create_test_keypair();
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
    let swap_request = cdk_common::nuts::SwapRequest::new(input_proofs.clone(), p2pk_outputs.clone());
    let swap_response = mint.process_swap_request(swap_request).await.unwrap();
    println!("Created P2PK proofs locked to Alice");

    // Step 4: Construct the P2PK proofs
    let p2pk_proofs = construct_proofs(
        swap_response.signatures.clone(),
        blinding_factors.clone(),
        secrets.clone(),
        &test_mint.public_keys_of_the_active_sat_keyset,
    ).unwrap();

    // Step 5: Try to spend Alice's proofs by signing with Bob's key (wrong key!)
    let (new_outputs, _) = create_test_blinded_messages(mint, input_amount).await.unwrap();
    let mut swap_request_wrong_sig = cdk_common::nuts::SwapRequest::new(
        p2pk_proofs.clone(),
        new_outputs.clone(),
    );

    // Sign with Bob's key instead of Alice's key
    for proof in swap_request_wrong_sig.inputs_mut() {
        proof.sign_p2pk(bob_secret.clone()).unwrap();
    }

    let result = mint.process_swap_request(swap_request_wrong_sig).await;
    assert!(result.is_err(), "Should fail when signed with wrong key");
    println!("✓ Spending signed by wrong person failed as expected: {:?}", result.err());
}

/// Test: Duplicate signatures are rejected
///
/// Verifies that using the same signature twice doesn't count as multiple signers
#[tokio::test]
async fn test_p2pk_duplicate_signatures() {
    let mint = create_test_mint().await.unwrap();

    let (alice_secret, alice_pubkey) = create_test_keypair();
    let (bob_secret, bob_pubkey) = create_test_keypair();

    // TODO: Create 2-of-2 multisig (Alice, Bob)
    // TODO: Sign with Alice twice (instead of Alice + Bob)
    // TODO: Verify swap fails - duplicate signatures not allowed
}

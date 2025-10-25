#![cfg(test)]
//! P2PK SIG_ALL tests for swap functionality
//!
//! These tests verify that the mint correctly enforces SIG_ALL flag behavior

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
        Some(Conditions::new(
            None, // no locktime
            None, // no additional pubkeys
            None, // no refund keys
            None, // default num_sigs (1)
            Some(SigFlag::SigAll), // SIG_ALL flag
            None, // no num_sigs_refund
        ).unwrap())
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

    // Step 6: Sign all proofs individually (SIG_INPUTS way) - should fail for SIG_ALL
    let mut swap_request_sig_inputs = cdk_common::nuts::SwapRequest::new(
        p2pk_proofs.clone(),
        new_outputs.clone(),
    );

    // Sign each proof individually (SIG_INPUTS mode)
    for proof in swap_request_sig_inputs.inputs_mut() {
        proof.sign_p2pk(alice_secret.clone()).unwrap();
    }

    let result = mint.process_swap_request(swap_request_sig_inputs).await;
    assert!(result.is_err(), "Should fail - SIG_INPUTS signatures not valid for SIG_ALL");
    println!("✓ Spending with SIG_INPUTS signatures failed as expected: {:?}", result.err());

    // Step 7: Sign the transaction with SIG_ALL (should succeed)
    let mut swap_request_with_sig = cdk_common::nuts::SwapRequest::new(
        p2pk_proofs.clone(),
        new_outputs.clone(),
    );

    // Use sign_sig_all to sign the transaction (signature goes on first proof's witness)
    swap_request_with_sig.sign_sig_all(alice_secret.clone()).unwrap();

    // Verify the signatures
    let verify_result = swap_request_with_sig.verify_sig_all();
    println!("verify_sig_all result: {:?}", verify_result);

    let result = mint.process_swap_request(swap_request_with_sig).await;
    assert!(result.is_ok(), "Should succeed with valid signature: {:?}", result.err());
    println!("✓ Spending WITH ALL signatures (SIG_ALL) succeeded");
}

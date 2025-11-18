#![cfg(test)]
//! Shared test helpers for spending condition tests (P2PK, HTLC, etc.)

use cdk_common::dhke::blind_message;
use cdk_common::nuts::nut10::Secret as Nut10Secret;
use cdk_common::nuts::{
    BlindedMessage, CurrencyUnit, Id, Keys, PublicKey, SecretKey, SpendingConditions,
};
use cdk_common::Amount;

use crate::mint::Mint;
use crate::secret::Secret;
use crate::test_helpers::mint::{create_test_mint, mint_test_proofs};
use crate::Error;

/// Test mint wrapper with convenient access to common keyset info
pub struct TestMintHelper {
    pub mint: Mint,
    pub active_sat_keyset_id: Id,
    pub public_keys_of_the_active_sat_keyset: Keys,
    /// Available denominations sorted largest first (e.g., [2147483648, 1073741824, ..., 2, 1])
    pub available_amounts_sorted: Vec<u64>,
}

impl TestMintHelper {
    pub async fn new() -> Result<Self, Error> {
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
        assert_eq!(
            active_sat_keyset.id, active_sat_keyset_id,
            "Keyset ID mismatch"
        );
        let public_keys_of_the_active_sat_keyset = active_sat_keyset.keys.clone();

        // Get the available denominations from the keyset, sorted largest first
        let mut available_amounts_sorted: Vec<u64> = public_keys_of_the_active_sat_keyset
            .iter()
            .map(|(amt, _)| amt.to_u64())
            .collect();
        available_amounts_sorted.sort_by(|a, b| b.cmp(a)); // Sort descending (largest first)

        Ok(TestMintHelper {
            mint,
            active_sat_keyset_id,
            public_keys_of_the_active_sat_keyset,
            available_amounts_sorted,
        })
    }

    /// Get a reference to the underlying mint
    pub fn mint(&self) -> &Mint {
        &self.mint
    }

    /// Split an amount into power-of-2 denominations
    /// Returns the amounts that sum to the total (e.g., 10 -> [8, 2])
    pub fn split_amount(&self, amount: Amount) -> Result<Vec<Amount>, Error> {
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
    pub async fn mint_proofs(&self, amount: Amount) -> Result<cdk_common::Proofs, Error> {
        let proofs = mint_test_proofs(&self.mint, amount).await?;

        // Build the split display string (e.g., "8+2")
        let split_amounts = self.split_amount(amount)?;
        let split_display: Vec<String> = split_amounts.iter().map(|a| a.to_string()).collect();
        println!("Minted {} sats [{}]", amount, split_display.join("+"));

        Ok(proofs)
    }

    /// Create a single blinded message with spending conditions for the given amount
    /// Returns (blinded_message, blinding_factor, secret)
    pub fn create_blinded_message(
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
pub fn create_test_keypair() -> (SecretKey, PublicKey) {
    let secret = SecretKey::generate();
    let pubkey = secret.public_key();
    (secret, pubkey)
}

/// Helper: Create a hash and preimage for testing
/// Returns (hash_hex_string, preimage_hex_string)
pub fn create_test_hash_and_preimage() -> (String, String) {
    use bitcoin::hashes::sha256::Hash as Sha256Hash;
    use bitcoin::hashes::Hash;

    // Create a 32-byte preimage
    let preimage_bytes = [0x42u8; 32];
    let hash = Sha256Hash::hash(&preimage_bytes);
    // Return hex-encoded hash and hex-encoded preimage
    (hash.to_string(), crate::util::hex::encode(preimage_bytes))
}

/// Helper: Unzip a vector of 3-tuples into 3 separate vectors
pub fn unzip3<A, B, C>(vec: Vec<(A, B, C)>) -> (Vec<A>, Vec<B>, Vec<C>) {
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

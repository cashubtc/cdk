//! NUT-14: Hashed Time Lock Contacts (HTLC)
//!
//! <https://github.com/cashubtc/nuts/blob/main/14.md>

use std::str::FromStr;

use bitcoin::secp256k1::schnorr::Signature;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::nut00::Witness;
use super::nut10::Secret;
use super::nut11::valid_signatures;
use super::{Conditions, Proof};
use crate::util::{hex, unix_time};

pub mod serde_htlc_witness;

/// NUT14 Errors
#[derive(Debug, Error)]
pub enum Error {
    /// Incorrect secret kind
    #[error("Secret is not a HTLC secret")]
    IncorrectSecretKind,
    /// HTLC locktime has already passed
    #[error("Locktime in past")]
    LocktimeInPast,
    /// Hash Required
    #[error("Hash required")]
    HashRequired,
    /// Hash is not valid
    #[error("Hash is not valid")]
    InvalidHash,
    /// Preimage does not match
    #[error("Preimage does not match")]
    Preimage,
    /// HTLC preimage must be valid hex encoding
    #[error("Preimage must be valid hex encoding")]
    InvalidHexPreimage,
    /// HTLC preimage must be exactly 32 bytes
    #[error("Preimage must be exactly 32 bytes (64 hex characters)")]
    PreimageInvalidSize,
    /// Witness Signatures not provided
    #[error("Witness did not provide signatures")]
    SignaturesNotProvided,
    /// SIG_ALL not supported in this context
    #[error("SIG_ALL proofs must be verified using a different method")]
    SigAllNotSupportedHere,
    /// Secp256k1 error
    #[error(transparent)]
    Secp256k1(#[from] bitcoin::secp256k1::Error),
    /// NUT11 Error
    #[error(transparent)]
    NUT11(#[from] super::nut11::Error),
    #[error(transparent)]
    /// Serde Error
    Serde(#[from] serde_json::Error),
}

/// HTLC Witness
#[derive(Default, Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct HTLCWitness {
    /// Preimage
    pub preimage: String,
    /// Signatures
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signatures: Option<Vec<String>>,
}

impl HTLCWitness {
    /// Decode the preimage from hex and verify it's exactly 32 bytes
    ///
    /// Returns the 32-byte preimage data if valid, or an error if:
    /// - The hex decoding fails
    /// - The decoded data is not exactly 32 bytes
    pub fn preimage_data(&self) -> Result<[u8; 32], Error> {
        const REQUIRED_PREIMAGE_BYTES: usize = 32;

        // Decode the 64-character hex string to bytes
        let preimage_bytes = hex::decode(&self.preimage).map_err(|_| Error::InvalidHexPreimage)?;

        // Verify the preimage is exactly 32 bytes
        if preimage_bytes.len() != REQUIRED_PREIMAGE_BYTES {
            return Err(Error::PreimageInvalidSize);
        }

        // Convert to fixed-size array
        let mut array = [0u8; 32];
        array.copy_from_slice(&preimage_bytes);
        Ok(array)
    }
}

impl Proof {
    /// Verify HTLC
    pub fn verify_htlc(&self) -> Result<(), Error> {
        let secret: Secret = self.secret.clone().try_into()?;
        let spending_conditions: Conditions = secret
            .secret_data()
            .tags()
            .cloned()
            .unwrap_or_default()
            .try_into()?;

        if spending_conditions.sig_flag == super::SigFlag::SigAll {
            return Err(Error::SigAllNotSupportedHere);
        }

        if secret.kind() != super::Kind::HTLC {
            return Err(Error::IncorrectSecretKind);
        }

        // Get the appropriate spending conditions based on locktime
        let now = unix_time();
        let requirements =
            super::nut10::get_pubkeys_and_required_sigs(&secret, now).map_err(Error::NUT11)?;

        // While a Witness is usually needed in a P2PK or HTLC proof, it's not
        // always needed. If we are past the locktime, and there are no refund
        // keys, then the proofs are anyone-can-spend:
        //     NUT-11: "If the tag locktime is the unix time and the mint's local
        //              clock is greater than locktime, the Proof becomes spendable
        //              by anyone, except if [there are no refund keys]"
        // Therefore, this function should not extract any Witness unless it
        // is needed to get a preimage or signatures.

        // If preimage is needed (before locktime), verify it
        if requirements.preimage_needed {
            // Extract HTLC witness
            let htlc_witness = match &self.witness {
                Some(Witness::HTLCWitness(witness)) => witness,
                _ => return Err(Error::IncorrectSecretKind),
            };

            // Verify preimage using shared function
            super::nut10::verify_htlc_preimage(htlc_witness, &secret)?;
        }

        if requirements.required_sigs == 0 {
            return Ok(());
        }

        // if we get here, the preimage check (if it was needed) has been done
        // and we know that at least one signature is required. So, we extract
        // the witness.signatures and count them:

        // Extract witness signatures
        let htlc_witness = match &self.witness {
            Some(Witness::HTLCWitness(witness)) => witness,
            _ => return Err(Error::IncorrectSecretKind),
        };
        let witness_signatures = htlc_witness
            .signatures
            .as_ref()
            .ok_or(Error::SignaturesNotProvided)?;

        // Convert signatures from strings
        let signatures: Vec<Signature> = witness_signatures
            .iter()
            .map(|s| Signature::from_str(s))
            .collect::<Result<Vec<_>, _>>()?;

        // Count valid signatures using relevant_pubkeys
        let msg: &[u8] = self.secret.as_bytes();
        let valid_sig_count = valid_signatures(msg, &requirements.pubkeys, &signatures)?;

        // Check if we have enough valid signatures
        if valid_sig_count >= requirements.required_sigs {
            Ok(())
        } else {
            Err(Error::NUT11(super::nut11::Error::SpendConditionsNotMet))
        }
    }

    /// Add Preimage
    #[inline]
    pub fn add_preimage(&mut self, preimage: String) {
        let signatures = self
            .witness
            .as_ref()
            .map(super::nut00::Witness::signatures)
            .unwrap_or_default();

        self.witness = Some(Witness::HTLCWitness(HTLCWitness {
            preimage,
            signatures,
        }))
    }
}

#[cfg(test)]
mod tests {
    use bitcoin::hashes::sha256::Hash as Sha256Hash;
    use bitcoin::hashes::Hash;

    use super::*;
    use crate::nuts::nut00::Witness;
    use crate::nuts::nut10::Kind;
    use crate::nuts::Nut10Secret;
    use crate::secret::Secret as SecretString;

    /// Tests that verify_htlc correctly accepts a valid HTLC with the correct preimage.
    ///
    /// This test ensures that a properly formed HTLC proof with the correct preimage
    /// passes verification.
    ///
    /// Mutant testing: Combined with negative tests, this catches mutations that
    /// replace verify_htlc with Ok(()) since the negative tests will fail.
    #[test]
    fn test_verify_htlc_valid() {
        // Create a valid HTLC secret with a known preimage (32 bytes)
        let preimage_bytes = [42u8; 32]; // 32-byte preimage
        let hash = Sha256Hash::hash(&preimage_bytes);
        let hash_str = hash.to_string();

        let nut10_secret = Nut10Secret::new(Kind::HTLC, hash_str, None::<Vec<Vec<String>>>);
        let secret: SecretString = nut10_secret.try_into().unwrap();

        let htlc_witness = HTLCWitness {
            preimage: hex::encode(&preimage_bytes),
            signatures: None,
        };

        let proof = Proof {
            amount: crate::Amount::from(1),
            keyset_id: crate::nuts::nut02::Id::from_str("00deadbeef123456").unwrap(),
            secret,
            c: crate::nuts::nut01::PublicKey::from_hex(
                "02a9acc1e48c25eeeb9289b5031cc57da9fe72f3fe2861d264bdc074209b107ba2",
            )
            .unwrap(),
            witness: Some(Witness::HTLCWitness(htlc_witness)),
            dleq: None,
        };

        // Valid HTLC should verify successfully
        assert!(proof.verify_htlc().is_ok());
    }

    /// Tests that verify_htlc correctly rejects an HTLC with a wrong preimage.
    ///
    /// This test is critical for security - if the verification function doesn't properly
    /// check the preimage against the hash, an attacker could spend HTLC-locked funds
    /// without knowing the correct preimage.
    ///
    /// Mutant testing: Catches mutations that replace verify_htlc with Ok(()) or remove
    /// the preimage verification logic.
    #[test]
    fn test_verify_htlc_wrong_preimage() {
        // Create an HTLC secret with a specific hash (32 bytes)
        let correct_preimage_bytes = [42u8; 32];
        let hash = Sha256Hash::hash(&correct_preimage_bytes);
        let hash_str = hash.to_string();

        let nut10_secret = Nut10Secret::new(Kind::HTLC, hash_str, None::<Vec<Vec<String>>>);
        let secret: SecretString = nut10_secret.try_into().unwrap();

        // Use a different preimage in the witness
        let wrong_preimage_bytes = [99u8; 32]; // Different from correct preimage
        let htlc_witness = HTLCWitness {
            preimage: hex::encode(&wrong_preimage_bytes),
            signatures: None,
        };

        let proof = Proof {
            amount: crate::Amount::from(1),
            keyset_id: crate::nuts::nut02::Id::from_str("00deadbeef123456").unwrap(),
            secret,
            c: crate::nuts::nut01::PublicKey::from_hex(
                "02a9acc1e48c25eeeb9289b5031cc57da9fe72f3fe2861d264bdc074209b107ba2",
            )
            .unwrap(),
            witness: Some(Witness::HTLCWitness(htlc_witness)),
            dleq: None,
        };

        // Verification should fail with wrong preimage
        let result = proof.verify_htlc();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::Preimage));
    }

    /// Tests that verify_htlc correctly rejects an HTLC with an invalid hash format.
    ///
    /// This test ensures that the verification function properly validates that the
    /// hash in the secret data is a valid SHA256 hash.
    ///
    /// Mutant testing: Catches mutations that replace verify_htlc with Ok(()) or
    /// remove the hash validation logic.
    #[test]
    fn test_verify_htlc_invalid_hash() {
        // Create an HTLC secret with an invalid hash (not a valid hex string)
        let invalid_hash = "not_a_valid_hash";

        let nut10_secret = Nut10Secret::new(
            Kind::HTLC,
            invalid_hash.to_string(),
            None::<Vec<Vec<String>>>,
        );
        let secret: SecretString = nut10_secret.try_into().unwrap();

        let preimage_bytes = [42u8; 32]; // Valid 32-byte preimage
        let htlc_witness = HTLCWitness {
            preimage: hex::encode(&preimage_bytes),
            signatures: None,
        };

        let proof = Proof {
            amount: crate::Amount::from(1),
            keyset_id: crate::nuts::nut02::Id::from_str("00deadbeef123456").unwrap(),
            secret,
            c: crate::nuts::nut01::PublicKey::from_hex(
                "02a9acc1e48c25eeeb9289b5031cc57da9fe72f3fe2861d264bdc074209b107ba2",
            )
            .unwrap(),
            witness: Some(Witness::HTLCWitness(htlc_witness)),
            dleq: None,
        };

        // Verification should fail with invalid hash
        let result = proof.verify_htlc();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::InvalidHash));
    }

    /// Tests that verify_htlc correctly rejects an HTLC with the wrong witness type.
    ///
    /// This test ensures that the verification function checks that the witness is
    /// of the correct type (HTLCWitness) and not some other witness type.
    ///
    /// Mutant testing: Catches mutations that replace verify_htlc with Ok(()) or
    /// remove the witness type check.
    #[test]
    fn test_verify_htlc_wrong_witness_type() {
        // Create an HTLC secret
        let preimage = "test_preimage";
        let hash = Sha256Hash::hash(preimage.as_bytes());
        let hash_str = hash.to_string();

        let nut10_secret = Nut10Secret::new(Kind::HTLC, hash_str, None::<Vec<Vec<String>>>);
        let secret: SecretString = nut10_secret.try_into().unwrap();

        // Create proof with wrong witness type (P2PKWitness instead of HTLCWitness)
        let proof = Proof {
            amount: crate::Amount::from(1),
            keyset_id: crate::nuts::nut02::Id::from_str("00deadbeef123456").unwrap(),
            secret,
            c: crate::nuts::nut01::PublicKey::from_hex(
                "02a9acc1e48c25eeeb9289b5031cc57da9fe72f3fe2861d264bdc074209b107ba2",
            )
            .unwrap(),
            witness: Some(Witness::P2PKWitness(super::super::nut11::P2PKWitness {
                signatures: vec![],
            })),
            dleq: None,
        };

        // Verification should fail with wrong witness type
        let result = proof.verify_htlc();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::IncorrectSecretKind));
    }

    /// Tests that add_preimage correctly adds a preimage to the proof.
    ///
    /// This test ensures that add_preimage actually modifies the witness and doesn't
    /// just return without doing anything.
    ///
    /// Mutant testing: Catches mutations that replace add_preimage with () without
    /// actually adding the preimage.
    #[test]
    fn test_add_preimage() {
        let preimage_bytes = [42u8; 32]; // 32-byte preimage
        let hash = Sha256Hash::hash(&preimage_bytes);
        let hash_str = hash.to_string();

        let nut10_secret = Nut10Secret::new(Kind::HTLC, hash_str, None::<Vec<Vec<String>>>);
        let secret: SecretString = nut10_secret.try_into().unwrap();

        let mut proof = Proof {
            amount: crate::Amount::from(1),
            keyset_id: crate::nuts::nut02::Id::from_str("00deadbeef123456").unwrap(),
            secret,
            c: crate::nuts::nut01::PublicKey::from_hex(
                "02a9acc1e48c25eeeb9289b5031cc57da9fe72f3fe2861d264bdc074209b107ba2",
            )
            .unwrap(),
            witness: None,
            dleq: None,
        };

        // Initially, witness should be None
        assert!(proof.witness.is_none());

        // Add preimage (hex-encoded)
        let preimage_hex = hex::encode(&preimage_bytes);
        proof.add_preimage(preimage_hex.clone());

        // After adding, witness should be Some with HTLCWitness
        assert!(proof.witness.is_some());
        if let Some(Witness::HTLCWitness(witness)) = &proof.witness {
            assert_eq!(witness.preimage, preimage_hex);
        } else {
            panic!("Expected HTLCWitness");
        }

        // The proof with added preimage should verify successfully
        assert!(proof.verify_htlc().is_ok());
    }

    /// Tests that verify_htlc requires BOTH locktime expired AND no refund keys for "anyone can spend".
    ///
    /// This test catches the mutation that replaces `&&` with `||` at line 83.
    /// The logic should be: (locktime expired AND no refund keys) → anyone can spend.
    /// If mutated to OR, it would allow spending when locktime passed even if refund keys exist.
    ///
    /// Mutant testing: Catches mutations that replace `&&` with `||` in the locktime check.
    #[test]
    fn test_htlc_locktime_and_refund_keys_logic() {
        use crate::nuts::nut01::PublicKey;
        use crate::nuts::nut11::Conditions;

        let preimage_bytes = [42u8; 32]; // 32-byte preimage
        let hash = Sha256Hash::hash(&preimage_bytes);
        let hash_str = hash.to_string();

        // Test: Locktime has passed (locktime=1) but refund keys ARE present
        // With correct logic (&&): Since refund_keys.is_none() is false, the "anyone can spend"
        //                          path is NOT taken, so signature is required
        // With mutation (||): Since locktime.lt(&unix_time()) is true, it WOULD take the
        //                     "anyone can spend" path immediately - WRONG!
        let refund_pubkey = PublicKey::from_hex(
            "02a9acc1e48c25eeeb9289b5031cc57da9fe72f3fe2861d264bdc074209b107ba2",
        )
        .unwrap();

        let conditions_with_refund = Conditions {
            locktime: Some(1), // Locktime in past (current time is much larger)
            pubkeys: None,
            refund_keys: Some(vec![refund_pubkey]), // Refund key present
            num_sigs: None,
            sig_flag: crate::nuts::nut11::SigFlag::default(),
            num_sigs_refund: None,
        };

        let nut10_secret = Nut10Secret::new(Kind::HTLC, hash_str, Some(conditions_with_refund));
        let secret: SecretString = nut10_secret.try_into().unwrap();

        let htlc_witness = HTLCWitness {
            preimage: hex::encode(&preimage_bytes),
            signatures: None, // No signature provided
        };

        let proof = Proof {
            amount: crate::Amount::from(1),
            keyset_id: crate::nuts::nut02::Id::from_str("00deadbeef123456").unwrap(),
            secret,
            c: crate::nuts::nut01::PublicKey::from_hex(
                "02a9acc1e48c25eeeb9289b5031cc57da9fe72f3fe2861d264bdc074209b107ba2",
            )
            .unwrap(),
            witness: Some(Witness::HTLCWitness(htlc_witness)),
            dleq: None,
        };

        // Should FAIL because even though locktime passed, refund keys are present
        // so the "anyone can spend" shortcut shouldn't apply. A signature is required.
        // With && this correctly fails. With || it would incorrectly pass.
        let result = proof.verify_htlc();
        assert!(
            result.is_err(),
            "Should fail when locktime passed but refund keys present without signature"
        );
    }
}

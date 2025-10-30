//! NUT-14: Hashed Time Lock Contacts (HTLC)
//!
//! <https://github.com/cashubtc/nuts/blob/main/14.md>

use std::str::FromStr;

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::schnorr::Signature;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::nut00::Witness;
use super::nut10::Secret;
use super::nut11::valid_signatures;
use super::{Conditions, Proof};
use crate::ensure_cdk;
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

impl Proof {
    /// Verify HTLC
    pub fn verify_htlc(&self) -> Result<(), Error> {
        let secret: Secret = self.secret.clone().try_into()?;
        let conditions: Option<Conditions> = secret
            .secret_data()
            .tags()
            .and_then(|c| c.clone().try_into().ok());

        let htlc_witness = match &self.witness {
            Some(Witness::HTLCWitness(witness)) => witness,
            _ => return Err(Error::IncorrectSecretKind),
        };

        const REQUIRED_PREIMAGE_BYTES: usize = 32;

        let preimage_bytes =
            hex::decode(&htlc_witness.preimage).map_err(|_| Error::InvalidHexPreimage)?;

        if preimage_bytes.len() != REQUIRED_PREIMAGE_BYTES {
            return Err(Error::PreimageInvalidSize);
        }

        if let Some(conditions) = conditions {
            // Check locktime
            if let Some(locktime) = conditions.locktime {
                // If locktime is in passed and no refund keys provided anyone can spend
                if locktime.lt(&unix_time()) && conditions.refund_keys.is_none() {
                    return Ok(());
                }

                // If refund keys are provided verify p2pk signatures
                if let (Some(refund_key), Some(signatures)) =
                    (conditions.refund_keys, &self.witness)
                {
                    let signatures = signatures
                        .signatures()
                        .ok_or(Error::SignaturesNotProvided)?
                        .iter()
                        .map(|s| Signature::from_str(s))
                        .collect::<Result<Vec<Signature>, _>>()?;

                    // If secret includes refund keys check that there is a valid signature
                    if valid_signatures(self.secret.as_bytes(), &refund_key, &signatures)?.ge(&1) {
                        return Ok(());
                    }
                }
            }
            // If pubkeys are present check there is a valid signature
            if let Some(pubkey) = conditions.pubkeys {
                let req_sigs = conditions.num_sigs.unwrap_or(1);

                let signatures = htlc_witness
                    .signatures
                    .as_ref()
                    .ok_or(Error::SignaturesNotProvided)?;

                let signatures = signatures
                    .iter()
                    .map(|s| Signature::from_str(s))
                    .collect::<Result<Vec<Signature>, _>>()?;

                let valid_sigs = valid_signatures(self.secret.as_bytes(), &pubkey, &signatures)?;
                ensure_cdk!(valid_sigs >= req_sigs, Error::IncorrectSecretKind);
            }
        }

        if secret.kind().ne(&super::Kind::HTLC) {
            return Err(Error::IncorrectSecretKind);
        }

        let hash_lock =
            Sha256Hash::from_str(secret.secret_data().data()).map_err(|_| Error::InvalidHash)?;

        let preimage_hash = Sha256Hash::hash(&preimage_bytes);

        if hash_lock.ne(&preimage_hash) {
            return Err(Error::Preimage);
        }

        Ok(())
    }

    /// Add Preimage
    #[inline]
    pub fn add_preimage(&mut self, preimage: String) {
        let signatures = self
            .witness
            .as_ref()
            .map(|w| w.signatures())
            .unwrap_or_default();

        self.witness = Some(Witness::HTLCWitness(HTLCWitness {
            preimage,
            signatures,
        }))
    }
}

#[cfg(test)]
mod tests {
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
        // Create a valid HTLC secret with a known preimage
        let preimage = "test_preimage";
        let hash = Sha256Hash::hash(preimage.as_bytes());
        let hash_str = hash.to_string();

        let nut10_secret = Nut10Secret::new(Kind::HTLC, hash_str, None::<Vec<Vec<String>>>);
        let secret: SecretString = nut10_secret.try_into().unwrap();

        let htlc_witness = HTLCWitness {
            preimage: preimage.to_string(),
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
        // Create an HTLC secret with a specific hash
        let correct_preimage = "correct_preimage";
        let hash = Sha256Hash::hash(correct_preimage.as_bytes());
        let hash_str = hash.to_string();

        let nut10_secret = Nut10Secret::new(Kind::HTLC, hash_str, None::<Vec<Vec<String>>>);
        let secret: SecretString = nut10_secret.try_into().unwrap();

        // Use a different preimage in the witness
        let htlc_witness = HTLCWitness {
            preimage: "wrong_preimage".to_string(),
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

        let htlc_witness = HTLCWitness {
            preimage: "some_preimage".to_string(),
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
        let preimage = "test_preimage";
        let hash = Sha256Hash::hash(preimage.as_bytes());
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

        // Add preimage
        proof.add_preimage(preimage.to_string());

        // After adding, witness should be Some with HTLCWitness
        assert!(proof.witness.is_some());
        if let Some(Witness::HTLCWitness(witness)) = &proof.witness {
            assert_eq!(witness.preimage, preimage);
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

        let preimage = "test_preimage";
        let hash = Sha256Hash::hash(preimage.as_bytes());
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
            preimage: preimage.to_string(),
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

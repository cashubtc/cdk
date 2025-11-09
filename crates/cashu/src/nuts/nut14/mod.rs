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
        let (preimage_needed, relevant_pubkeys, relevant_num_sigs_required) =
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
        if preimage_needed {
            // Extract HTLC witness
            let htlc_witness = match &self.witness {
                Some(Witness::HTLCWitness(witness)) => witness,
                _ => return Err(Error::IncorrectSecretKind),
            };

            // Verify preimage using shared function
            super::nut10::verify_htlc_preimage(htlc_witness, &secret)?;
        }

        if relevant_num_sigs_required == 0 {
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
        let valid_sig_count = valid_signatures(msg, &relevant_pubkeys, &signatures)?;

        // Check if we have enough valid signatures
        if valid_sig_count >= relevant_num_sigs_required {
            Ok(())
        } else {
            Err(Error::IncorrectSecretKind)
        }
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

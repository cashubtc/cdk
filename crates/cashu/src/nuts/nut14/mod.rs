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
use crate::util::unix_time;

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

/// Get the relevant public keys and required signature count for HTLC verification
///
/// Takes into account locktime - if locktime has passed, returns refund keys,
/// otherwise returns primary pubkeys. Returns (preimage_needed, pubkeys, required_sigs).
pub fn get_pubkeys_and_required_sigs_for_htlc(
    secret: &Secret,
    current_time: u64,
) -> Result<(bool, Vec<super::nut01::PublicKey>, u64), Error> {
    use super::nut10::Kind;

    debug_assert!(
        secret.kind() == Kind::HTLC,
        "get_pubkeys_and_required_sigs_for_htlc called with non-HTLC secret - this is a bug"
    );

    let conditions: Conditions = secret
        .secret_data()
        .tags()
        .cloned()
        .unwrap_or_default()
        .try_into()?;

    // Check if locktime has passed
    let locktime_passed = conditions
        .locktime
        .map(|locktime| locktime < current_time)
        .unwrap_or(false);

    // Determine spending path based on locktime
    let (preimage_needed, pubkeys, required_sigs) = if locktime_passed {
        // After locktime: use refund path (no preimage needed)
        if let Some(refund_keys) = &conditions.refund_keys {
            // Locktime has passed and refund keys exist - use refund keys
            let refund_sigs = conditions.num_sigs_refund.unwrap_or(1);
            (false, refund_keys.clone(), refund_sigs)
        } else {
            // Locktime has passed with no refund keys - anyone can spend
            (false, vec![], 0)
        }
    } else {
        // Before locktime: use hash path (preimage needed)
        // Get pubkeys from conditions (for HTLC, data contains hash, not pubkey)
        let pubkeys = conditions.pubkeys.clone().unwrap_or_default();
        let required_sigs = conditions.num_sigs.unwrap_or(1);
        (true, pubkeys, required_sigs)
    };

    Ok((preimage_needed, pubkeys, required_sigs))
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

        debug_assert!(
            spending_conditions.sig_flag != super::SigFlag::SigAll,
            "verify_htlc called with SIG_ALL proof - this is a bug"
        );

        debug_assert!(
            secret.kind() == super::Kind::HTLC,
            "verify_htlc called with non-HTLC secret - this is a bug"
        );

        // Get the appropriate spending conditions based on locktime
        let now = unix_time();
        let (preimage_needed, relevant_pubkeys, relevant_num_sigs_required) =
            get_pubkeys_and_required_sigs_for_htlc(&secret, now)?;

        // don't extract the witness until it's needed. Remember a post-locktime
        // zero-refunds proof is acceptable here, and therefore a Witness isn't always
        // needed

        // If preimage is needed (before locktime), verify it
        if preimage_needed {
            let hash_lock =
                Sha256Hash::from_str(secret.secret_data().data()).map_err(|_| Error::InvalidHash)?;
            // Extract HTLC witness
            let htlc_witness = match &self.witness {
                Some(Witness::HTLCWitness(witness)) => witness,
                _ => return Err(Error::IncorrectSecretKind),
            };
            let hash_of_preimage = Sha256Hash::hash(htlc_witness.preimage.as_bytes());

            if hash_lock.ne(&hash_of_preimage) {
                return Err(Error::Preimage);
            }
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

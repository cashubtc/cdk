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

        let preimage_hash = Sha256Hash::hash(htlc_witness.preimage.as_bytes());

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

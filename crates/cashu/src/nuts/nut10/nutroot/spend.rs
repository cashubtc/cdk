use core::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{tagged_hash, Error, Transaction, Witness};
use crate::nuts::{KeySetVersion, Proof, ProofState, PublicKey, State};
use crate::util::hex;

/// Durable opening for a v3 spend commitment. Never publish it indiscriminately.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpendRecord {
    /// Per-input transaction digest.
    pub input_digest: [u8; 32],
    /// Exact UTF-8 witness string accepted with the request.
    pub witness: String,
    /// Whether the exercised leaf requires public disclosure.
    pub disclosure: bool,
}

impl fmt::Debug for SpendRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpendRecord")
            .field("opening", &"[REDACTED]")
            .field("disclosure", &self.disclosure)
            .finish()
    }
}

/// Bind a spent proof, its input digest, and the exact accepted witness bytes.
pub fn spend_commitment(y: &PublicKey, digest: [u8; 32], witness: &str) -> [u8; 32] {
    let mut message = y.to_bytes();
    message.extend_from_slice(&digest);
    message.extend_from_slice(&Sha256::digest(witness.as_bytes()));
    tagged_hash("Cashu_SpendCommitment", &message)
}

impl SpendRecord {
    /// Verify and record a v3 authorization, including the exercised disclosure mode.
    pub fn new(
        proof: &Proof,
        transaction: &Transaction,
        index: usize,
        now: u64,
    ) -> Result<Self, Error> {
        if proof.keyset_id.get_version() != KeySetVersion::Version02 {
            return Err(Error::InvalidKeyset);
        }
        let witness = proof.witness.as_ref().ok_or(Error::InvalidWitness)?;
        let raw = match witness {
            crate::nuts::Witness::NutrootWitness(raw) => raw.clone(),
            other => serde_json::from_str::<String>(
                &serde_json::to_string(other).map_err(|_| Error::InvalidWitness)?,
            )
            .map_err(|_| Error::InvalidWitness)?,
        };
        if raw.len() > 4096 {
            return Err(Error::InvalidWitness);
        }
        let witness: Witness = serde_json::from_str(&raw).map_err(|_| Error::InvalidWitness)?;
        let input_digest = transaction.input_digest(index)?;
        if transaction.proof_digest(proof)? != input_digest {
            return Err(Error::InvalidTransaction);
        }
        let disclosure = witness.verify(&proof.secret.to_string(), input_digest, now)?;
        Ok(Self {
            input_digest,
            witness: raw,
            disclosure,
        })
    }

    /// Apply the NUT-07/NUT-17 disclosure rules. Pending/unspent entries expose nothing.
    pub fn apply_to(&self, state: &mut ProofState) {
        state.witness = None;
        state.input_digest = None;
        state.commitment = None;
        if state.state == State::Spent {
            state.commitment = Some(hex::encode(spend_commitment(
                &state.y,
                self.input_digest,
                &self.witness,
            )));
            if self.disclosure {
                state.input_digest = Some(hex::encode(self.input_digest));
                state.witness = Some(crate::nuts::Witness::NutrootWitness(self.witness.clone()));
            }
        }
    }
}

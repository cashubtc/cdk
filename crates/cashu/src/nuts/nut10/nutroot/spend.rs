use core::fmt;

use serde::{Deserialize, Serialize};

use super::{Error, Transaction, Witness};
use crate::nuts::{KeySetVersion, Proof, ProofState, State};
use crate::util::hex;

/// The witness a v3 input was accepted with, and the input digest it signs.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpendRecord {
    /// Per-input transaction digest.
    pub input_digest: [u8; 32],
    /// Exact UTF-8 witness string accepted with the request.
    pub witness: String,
}

impl fmt::Debug for SpendRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpendRecord")
            .field("input_digest", &hex::encode(self.input_digest))
            .finish()
    }
}

impl SpendRecord {
    /// Verify and record a v3 authorization.
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
        witness.verify(&proof.secret.to_string(), input_digest, now)?;
        Ok(Self {
            input_digest,
            witness: raw,
        })
    }

    /// Apply NUT-07/NUT-17: a v3 witness and its input digest are public from `PENDING` on.
    pub fn apply_to(&self, state: &mut ProofState) {
        let published = matches!(state.state, State::Pending | State::Spent);
        state.input_digest = published.then(|| hex::encode(self.input_digest));
        state.witness =
            published.then(|| crate::nuts::Witness::NutrootWitness(self.witness.clone()));
    }
}

//! NUT-xx: Cairo Contracts (CC)
//!
//! <https://github.com/cashubtc/nuts/blob/main/xx.md>

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use serde::{Deserialize, Serialize};
use starknet_types_core::felt::Felt;
use thiserror::Error;

// use super::nut00::Witness;
use super::nut01::PublicKey;
use super::{Conditions, Proof};
use crate::nuts::nut00::BlindedMessage;
// use stwo_cairo_prover::stwo::core::vcs::blake2_merkle::{
//     Blake2sMerkleChannel, Blake2sMerkleHasher,
// };

pub mod serde_cc_witness;

/// Nutxx Error
#[derive(Debug, Error)]
pub enum Error {
    /// Incorrect secret kind
    #[error("Secret is not a cc secret")]
    IncorrectSecretKind,
    /// CC locktime has already passed
    #[error("Locktime in past")]
    LocktimeInPast,
    /// Not implemented
    #[error("Not implemented")]
    NotImplemented,
}

/// CC Witness
#[derive(Default, Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]

/// The Witness of a Cairo program
///
/// Given to the mint by the recipient
pub struct CCWitness {
    /// The serialized .json proof
    pub proof: String,
}

impl CCWitness {
    #[inline]
    /// Check if Witness is empty
    pub fn is_empty(&self) -> bool {
        self.proof == ""
    }
}

impl Proof {
    // prove cairo program
    pub fn prove_cc(&self) -> Result<CCWitness, Error> {
        Err(Error::NotImplemented)
    }

    /// Verify CC
    pub fn verify_cc(&self) -> Result<(), Error> {
        Err(Error::NotImplemented)
    }
}

impl BlindedMessage {}

/// Spending Conditions
///
/// Defined in [NUT10](https://github.com/cashubtc/nuts/blob/main/10.md)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SpendingConditions {
    // /// NUT11 Spending conditions
    // ///
    // /// Defined in [NUT11](https://github.com/cashubtc/nuts/blob/main/11.md)
    // P2PKConditions {
    //     /// The public key of the recipient of the locked ecash
    //     data: PublicKey,
    //     /// Additional Optional Spending [`Conditions`]
    //     conditions: Option<Conditions>,
    // },
    // /// NUT14 Spending conditions
    // ///
    // /// Defined in [NUT14](https://github.com/cashubtc/nuts/blob/main/14.md)
    // HTLCConditions {
    //     /// Hash Lock of ecash
    //     data: Sha256Hash,
    //     /// Additional Optional Spending [`Conditions`]
    //     conditions: Option<Conditions>,
    // },
    /// NUTXX Spending conditions
    /// Defined in [NUTXX](https://github.com/cashubtc/nuts/blob/main/xx.md)
    CCConditions {
        /// Program hash
        data: Felt,
        /// Additional Optional Spending [`Conditions`]
        conditions: Option<Conditions>,
    },
}

impl SpendingConditions {
    /// Create a new CC spending condition
    pub fn new_cc_conditions(program_hash: Felt, conditions: Option<Conditions>) -> Self {
        Self::CCConditions {
            data: program_hash,
            conditions,
        }
    }
}

impl SpendingConditions {
    pub fn new_cc(program_hash: Felt, output: Vec<Felt>) -> Result<Self, Error> {
        Ok(Self::Cairo {
            data: program_hash,
            conditions: Some(CairoConditions { output }),
        })
    }
    pub fn kind(&self) -> Kind {
        match self {
            Self::CCConditions { .. } => Kind::CC,
        }
    }
}

// impl TryFrom<&Secret> for SpendingConditions {}
// impl TryFrom<Nut10Secret> for SpendingConditions {}
// impl From<SpendingConditions> for super::nut10::Secret {}

#[cfg(test)]
mod tests {
    // TODO: write tests

    #[test]
    fn test_secret_ser() {
        // testing the serde serialization of the secret
        // 1. Create a secret
        // 2. Serialize it to JSON
        // 3. Deserialize it back to a secret
        // 4. Assert that the original secret and the deserialized secret are equal

        let data: Felt = _; // some hash output from the correct hash function
    }

    #[test]
    fn test_witness_cc() {
        // testing the creation of a CC witness
        // 1. Create a CC secret
        // 2. Generate a witness (stark proofs) for the CC
        // 3. Verify the witness
    }

    #[test]
    fn test_verify_soundness() {
        // testing the verification of an invalid CC proof
        // 1. Create an invalid CC secret
        // 2. Generate a proof for the CC
        // 3. Verify the proof
        // 4. Assert that the proof is valid
    }
}

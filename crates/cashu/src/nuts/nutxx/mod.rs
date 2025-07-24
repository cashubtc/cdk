//! NUT-xx: Cairo Contracts (CC)
//!
//! <https://github.com/cashubtc/nuts/blob/main/xx.md>

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use serde::{Deserialize, Serialize};
use thiserror::Error;

// use super::nut00::Witness;
use super::nut01::PublicKey;
use super::{Conditions, Proof};
use crate::nuts::nut00::BlindedMessage;

use starknet_types_core::felt::Felt;
use stwo_cairo_prover::stwo::core::vcs::blake2_merkle::{
    Blake2sMerkleChannel, Blake2sMerkleHasher,
};

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
    /// NUT11 Spending conditions
    ///
    /// Defined in [NUT11](https://github.com/cashubtc/nuts/blob/main/11.md)
    P2PKConditions {
        /// The public key of the recipient of the locked ecash
        data: PublicKey,
        /// Additional Optional Spending [`Conditions`]
        conditions: Option<Conditions>,
    },
    /// NUT14 Spending conditions
    ///
    /// Defined in [NUT14](https://github.com/cashubtc/nuts/blob/main/14.md)
    HTLCConditions {
        /// Hash Lock of ecash
        data: Sha256Hash,
        /// Additional Optional Spending [`Conditions`]
        conditions: Option<Conditions>,
    },
    /// NUTXX Spending conditions
    /// Defined in [NUTXX](https://github.com/cashubtc/nuts/blob/main/xx.md)
    CCConditions {
        /// Program hash
        data: Felt,
        /// Additional Optional Spending [`Conditions`]
        conditions: Option<Conditions>,
    },
}

impl SpendingConditions {}

// impl TryFrom<&Secret> for SpendingConditions {}
// impl TryFrom<Nut10Secret> for SpendingConditions {}
// impl From<SpendingConditions> for super::nut10::Secret {}

#[cfg(test)]
mod tests {
    // TODO: write tests
}

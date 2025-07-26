//! NUT-xx: Cairo Contracts (CC)
//!
//! <https://github.com/cashubtc/nuts/blob/main/xx.md>

use bitcoin::hashes::sha256::Hash as Sha256Hash;
// use cairo_air::utils::{serialize_proof_to_file, ProofFormat};
use cairo_air::verifier::verify_cairo;
use cairo_air::{CairoProof, PreProcessedTraceVariant};
use serde::{Deserialize, Serialize};
use starknet_types_core::felt::Felt;
use stwo_cairo_prover::stwo::core::vcs::blake2_merkle::{
    Blake2sMerkleChannel, Blake2sMerkleHasher,
};
use thiserror::Error;

// use super::nut00::Witness;
use super::nut01::PublicKey;
use super::{Conditions, Proof};
use crate::nuts::nut00::BlindedMessage;

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
        // let secret: Nut10Secret = self.secret.clone().try_into()?;
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

fn verify_cc_test(
    _secret_data: String, // TODO: verify this also, should have type `SecretData`
    witness: &CCWitness,
    with_pedersen: bool,
) -> Result<(), Error> {
    // info!("Verifying proof from: {:?}", proof);
    let cairo_proof = serde_json::from_str::<CairoProof<Blake2sMerkleHasher>>(&witness.proof)
        .expect("Failed to deserialize Cairo proof");
    let preprocessed_trace = match with_pedersen {
        true => PreProcessedTraceVariant::Canonical,
        false => PreProcessedTraceVariant::CanonicalWithoutPedersen,
    };
    let result = verify_cairo::<Blake2sMerkleChannel>(cairo_proof, preprocessed_trace);
    match result {
        Ok(_) => Ok(()),
        Err(_) => Err(Error::NotImplemented), // TODO: find better error
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    // use crate::nuts::nut00::{Amount, Id, Secret, Witness};

    #[test]
    fn test_verify() {
        let proof_json = {
            let path = PathBuf::from("./example_proof.json");
            std::fs::read_to_string(path).unwrap()
        };
        let witness = CCWitness { proof: proof_json };
        verify_cc_test("".to_string(), &witness, false).unwrap();
        assert!(verify_cc_test("".to_string(), &witness, true).is_ok());

        // TODO: make it work with the real verify_cc:
        // let valid_proof: Proof = Proof {
        //     amount: Amount::from(100),
        //     keyset_id: Id::from_str("009a1f293253e41e").unwrap(),
        //     secret: Secret::generate(),
        //     c: PublicKey::from_str(
        //         "02bc9097997d81afb2cc7346b5e4345a9346bd2a506eb7958598a72f0cf85163ea",
        //     )
        //     .unwrap(),
        //     witness: Some(Witness::CCWitness(witness)),
        //     dleq: None,
        // };
        // valid_proof.verify_cc().unwrap();
        // assert!(valid_proof.verify_cc().is_ok());

        // let invalid_proof: Proof = // TODO: example of an invalid proof
        // assert!(invalid_proof.verify_cc().is_err());
    }
    // TODO: write tests
}

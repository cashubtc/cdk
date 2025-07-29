//! NUT-xx: STARK-proven Computations (Cairo)
//!
//! <https://github.com/cashubtc/nuts/blob/main/xx.md>

// use cairo_air::utils::{serialize_proof_to_file, ProofFormat};
use cairo_air::verifier::{verify_cairo, CairoVerificationError};
use cairo_air::{CairoProof, PreProcessedTraceVariant};
use serde::{Deserialize, Serialize};
use stwo_cairo_prover::stwo_prover::core::fri::FriConfig;
use stwo_cairo_prover::stwo_prover::core::pcs::PcsConfig;
// use starknet_types_core::felt::Felt;
use stwo_cairo_prover::stwo_prover::core::vcs::blake2_merkle::{
    Blake2sMerkleChannel, Blake2sMerkleHasher,
};
use thiserror::Error;

use super::nut00::Witness;
// use super::nut11::Conditions;
// use super::{Kind, Nut10Secret, Proof, Proofs, SecretKey};
use super::{Nut10Secret, Proof};

pub mod serde_cairo_witness;

/// Nutxx Error
#[derive(Debug, Error)]
pub enum Error {
    /// Incorrect secret kind
    #[error("Secret is not a Cairo secret")]
    IncorrectSecretKind,
    /// Cairo verification error
    #[error(transparent)]
    CairoVerification(CairoVerificationError),
    /// NUT11 Error
    #[error(transparent)]
    NUT11(#[from] super::nut11::Error),
    /// Serde Error
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
    /// Not implemented
    #[error("Not implemented")]
    NotImplemented,
}

/// Cairo Witness
#[derive(Default, Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]

/// The Witness of a Cairo program
///
/// Given to the mint by the recipient
pub struct CairoWitness {
    /// The serialized .json proof
    pub proof: String,
}

impl CairoWitness {
    #[inline]
    /// Check if Witness is empty
    pub fn is_empty(&self) -> bool {
        self.proof == ""
    }
}

fn secure_pcs_config() -> PcsConfig {
    PcsConfig {
        pow_bits: 26,
        fri_config: FriConfig {
            log_last_layer_degree_bound: 0,
            log_blowup_factor: 1,
            n_queries: 70,
        },
    }
}

impl Proof {
    /// Verify Cairo
    pub fn verify_cairo(&self) -> Result<(), Error> {
        let secret: Nut10Secret = self.secret.clone().try_into()?;
        let cairo_witness = match &self.witness {
            Some(Witness::CairoWitness(witness)) => witness,
            _ => return Err(Error::IncorrectSecretKind),
        };

        // let conditions: Option<Conditions> = secret
        //     .secret_data()
        //     .tags()
        //     .and_then(|c| c.clone().try_into().ok());

        // if let Some(_conditions) = conditions {
        //     // additional conditions are not yet supported with Cairo
        //     return Err(Error::NotImplemented);
        // }

        if secret.kind().ne(&super::Kind::Cairo) {
            return Err(Error::IncorrectSecretKind);
        }

        // TODO: verify program (secret)

        let cairo_proof =
            match serde_json::from_str::<CairoProof<Blake2sMerkleHasher>>(&cairo_witness.proof) {
                Ok(proof) => proof,
                Err(e) => return Err(Error::Serde(e)),
            };

        let preprocessed_trace = PreProcessedTraceVariant::CanonicalWithoutPedersen; // TODO: give option
        let result = verify_cairo::<Blake2sMerkleChannel>(
            cairo_proof,
            secure_pcs_config(),
            preprocessed_trace,
        );
        match result {
            Ok(_) => Ok(()),
            Err(e) => Err(Error::CairoVerification(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::convert::TryInto;
    use std::str::FromStr;

    use super::*;
    use crate::secret::Secret;
    use crate::{Amount, Conditions, Id, Kind, Nut10Secret, SecretKey, SigFlag};

    #[test]
    fn test_verify() {
        let cairo_proof = include_str!("example_proof.json").to_string();
        // println!("cairo proof: {}", cairo_proof);
        let witness = CairoWitness { proof: cairo_proof };

        let secret_key =
            SecretKey::from_str("99590802251e78ee1051648439eedb003dc539093a48a44e7b8f2642c909ea37")
                .unwrap();
        let v_key = secret_key.public_key();

        let conditions = Conditions {
            locktime: None,
            pubkeys: None,
            refund_keys: None,
            num_sigs: None,
            sig_flag: SigFlag::SigInputs,
            num_sigs_refund: None,
        };

        let secret: Secret = Nut10Secret::new(
            Kind::Cairo,
            "PROGRAM_HASH_TODO".to_string(),
            Some(conditions), // TODO: adapt conditions to Cairo
        )
        .try_into()
        .unwrap();

        let valid_proof: Proof = Proof {
            amount: Amount::ZERO,
            keyset_id: Id::from_str("009a1f293253e41e").unwrap(),
            secret,
            c: v_key, // TODO: this serves no purpose for now
            witness: Some(Witness::CairoWitness(witness)),
            dleq: None,
        };
        valid_proof.verify_cairo().unwrap();
        assert!(valid_proof.verify_cairo().is_ok());

        // let invalid_proof: Proof = // TODO: example of an invalid proof
        // assert!(invalid_proof.verify_cc().is_err());
    }
}

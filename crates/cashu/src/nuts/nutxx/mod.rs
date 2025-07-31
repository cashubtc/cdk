//! NUT-xx: STARK-proven Computations (Cairo)
//!
//! <https://github.com/cashubtc/nuts/blob/main/xx.md>

use cairo_air::air::PubMemoryValue;
use cairo_air::verifier::{verify_cairo, CairoVerificationError};
use cairo_air::{CairoProof, PreProcessedTraceVariant};
use serde::{Deserialize, Serialize};
use starknet_types_core::felt::Felt;
use stwo_cairo_prover::stwo_prover::core::fri::FriConfig;
use stwo_cairo_prover::stwo_prover::core::pcs::PcsConfig;
use stwo_cairo_prover::stwo_prover::core::vcs::blake2_merkle::{
    Blake2sMerkleChannel, Blake2sMerkleHasher,
};
use stwo_cairo_prover::stwo_prover::core::vcs::blake3_hash::{Blake3Hash, Blake3Hasher};
use thiserror::Error;

use super::nut00::Witness;
use super::{Conditions, Nut10Secret, Proof};

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
    /// Program hash verification error
    #[error("Program hash from proof does not match program hash from secret")]
    ProgramHashVerification,
    /// Output verification error
    #[error("Output from proof does not match output from secret")]
    OutputVerification,
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

fn pmv_to_felt(pmv: &PubMemoryValue) -> Felt {
    let (_id, value) = pmv;
    let mut le_bytes = [0u8; 32];
    for (i, &v) in value.iter().enumerate() {
        let start = i * 4;
        le_bytes[start..start + 4].copy_from_slice(&v.to_le_bytes());
    }
    Felt::from_bytes_le(&le_bytes)
}

/// TODO: use something more secure for hashing multiple values,
/// like `poseidon_hash_many` on the Felt values directly
fn hash_bytecode(bytecode: &Vec<Felt>) -> Blake3Hash {
    let mut hasher = Blake3Hasher::default();
    for felt in bytecode {
        for byte in felt.to_bytes_le().iter() {
            hasher.update(&[*byte]);
        }
    }
    hasher.finalize()
}

impl Proof {
    /// Verify Cairo
    pub fn verify_cairo(&self) -> Result<(), Error> {
        let secret: Nut10Secret = self.secret.clone().try_into()?;
        let cairo_witness = match &self.witness {
            Some(Witness::CairoWitness(witness)) => witness,
            _ => return Err(Error::IncorrectSecretKind),
        };

        let conditions: Option<Conditions> = secret
            .secret_data()
            .tags()
            .and_then(|c| c.clone().try_into().ok());

        if let Some(_conditions) = conditions {
            // TODO: additional conditions are not yet supported with Cairo
            return Err(Error::NotImplemented);
        }

        if secret.kind().ne(&super::Kind::Cairo) {
            return Err(Error::IncorrectSecretKind);
        }

        let cairo_proof =
            match serde_json::from_str::<CairoProof<Blake2sMerkleHasher>>(&cairo_witness.proof) {
                Ok(proof) => proof,
                Err(e) => return Err(Error::Serde(e)),
            };

        let program: &Vec<PubMemoryValue> = &cairo_proof.claim.public_data.public_memory.program;
        let bytecode = program.iter().map(|v| pmv_to_felt(v)).collect::<Vec<_>>();
        let program_hash = hash_bytecode(&bytecode);

        if program_hash.to_string() != secret.secret_data().data() {
            return Err(Error::ProgramHashVerification);
        }

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

    use serde::de::{self, Deserializer};
    use starknet_types_core::felt::Felt;

    use super::*;
    use crate::secret::Secret;
    use crate::{Amount, Conditions, Id, Kind, Nut10Secret, PublicKey, SecretKey, SigFlag};

    #[derive(Deserialize)]
    struct Executable {
        program: Program,
    }

    #[derive(Deserialize)]
    struct Program {
        #[serde(deserialize_with = "deserialize_felt_vec")]
        bytecode: Vec<Felt>,
    }

    fn deserialize_felt_vec<'de, D>(deserializer: D) -> Result<Vec<Felt>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let hex_strings: Vec<String> = Vec::deserialize(deserializer)?;
        hex_strings
            .into_iter()
            .map(|s| {
                // TODO: this is a hack because `Felt::from_hex` doesn't work with negative numbers
                let is_negative = s.starts_with('-');
                let normalized_hex = if is_negative {
                    s.trim_start_matches('-').to_string()
                } else {
                    s.clone()
                };
                let felt = Felt::from_hex(&normalized_hex).map_err(de::Error::custom)?;
                let corrected_felt = if is_negative { -felt } else { felt };
                Ok(corrected_felt)
            })
            .collect()
    }

    #[test]
    fn test_verify() {
        let cairo_proof = include_str!("example_proof.json").to_string();
        let witness = CairoWitness { proof: cairo_proof };

        let secret_key =
            SecretKey::from_str("99590802251e78ee1051648439eedb003dc539093a48a44e7b8f2642c909ea37")
                .unwrap();
        let v_key = secret_key.public_key();

        // let conditions = Conditions {
        //     locktime: None,
        //     pubkeys: None,
        //     refund_keys: None,
        //     num_sigs: None,
        //     sig_flag: SigFlag::SigInputs,
        //     num_sigs_refund: None,
        // };

        // hash the program bytecode
        let executable_json = include_str!("example_executable.json");
        let executable: Executable = serde_json::from_str(executable_json).unwrap();
        let program_hash = hash_bytecode(&executable.program.bytecode);

        let secret: Secret = Nut10Secret::new(
            Kind::Cairo,
            program_hash.to_string(),
            // Some(conditions), // TODO: adapt conditions to Cairo
            None::<Conditions>,
        )
        .try_into()
        .unwrap();

        let valid_proof: Proof = Proof {
            amount: Amount::ZERO,
            keyset_id: Id::from_str("009a1f293253e41e").unwrap(), // TODO: check how this is used
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

    #[test]
    fn test_secret_ser() {
        // testing the serde serialization of the secret
        let conditions = Conditions {
            locktime: Some(99999),
            pubkeys: Some(vec![
                PublicKey::from_str(
                    "02698c4e2b5f9534cd0687d87513c759790cf829aa5739184a3e3735471fbda904",
                )
                .unwrap(),
                PublicKey::from_str(
                    "023192200a0cfd3867e48eb63b03ff599c7e46c8f4e41146b2d281173ca6c50c54",
                )
                .unwrap(),
            ]),
            refund_keys: Some(vec![PublicKey::from_str(
                "033281c37677ea273eb7183b783067f5244933ef78d8c3f15b1a77cb246099c26e",
            )
            .unwrap()]),
            num_sigs: Some(2),
            sig_flag: SigFlag::SigAll,
            num_sigs_refund: None,
        };

        let data = Felt::from_hex("0x1234567890abcdef").unwrap();

        let secret = Nut10Secret::new(Kind::Cairo, data.to_hex_string(), Some(conditions));

        let secret_str = serde_json::to_string(&secret).unwrap();

        let secret_der: Nut10Secret = serde_json::from_str(&secret_str).unwrap();

        assert_eq!(secret, secret_der);
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

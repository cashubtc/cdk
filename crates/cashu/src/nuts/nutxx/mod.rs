//! NUT-xx: STARK-proven Computations (Cairo)
//!
//! <https://github.com/cashubtc/nuts/blob/main/xx.md>

use std::str::FromStr;

use cairo_air::air::PubMemoryValue;
use cairo_air::verifier::{verify_cairo, CairoVerificationError};
use cairo_air::{CairoProof, PreProcessedTraceVariant};
use serde::{Deserialize, Serialize};
use starknet_types_core::felt::Felt;
use starknet_types_core::hash::{Poseidon, StarkHash};
use stwo_cairo_prover::stwo_prover::core::fri::FriConfig;
use stwo_cairo_prover::stwo_prover::core::pcs::PcsConfig;
use stwo_cairo_prover::stwo_prover::core::vcs::blake2_merkle::{
    Blake2sMerkleChannel, Blake2sMerkleHasher,
};
use thiserror::Error;

use super::nut00::Witness;
use super::{Nut10Secret, Proof};
use crate::nut11::TagKind;

pub mod serde_cairo_witness;

/// Nutxx Error
#[derive(Debug, Error)]
pub enum Error {
    /// Incorrect secret kind
    #[error("Secret is not a Cairo secret")]
    IncorrectSecretKind,
    /// Incorrect witness kind
    #[error("Witness is not a Cairo witness")]
    IncorrectWitnessKind,
    /// Cairo verification error
    #[error(transparent)]
    CairoVerification(CairoVerificationError),
    /// Program hash verification error
    #[error("Program hash from proof \"{0}\" does not match program hash from secret \"{1}\"")]
    ProgramHashVerification(Felt, Felt),
    /// Output verification error
    #[error("Output hash from proof \"{0}\" does not match output hash from secret \"{1}\"")]
    OutputHashVerification(Felt, Felt),
    /// Felt from string error
    #[error(transparent)]
    FeltFromStr(<Felt as std::str::FromStr>::Err),
    /// Serde Error
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
    /// Not implemented
    #[error("Not implemented")]
    NotImplemented,
}

/// Cairo spending conditions
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Conditions {
    /// Hash of the output of the program
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<Felt>,
}

impl From<Conditions> for Vec<Vec<String>> {
    fn from(conditions: Conditions) -> Vec<Vec<String>> {
        let Conditions { output } = conditions;

        let mut tags = Vec::new();

        if let Some(output) = output {
            tags.push(vec![
                TagKind::Custom("program_output".to_string()).to_string(),
                output.to_string(),
            ]);
        }
        tags
    }
}

impl TryFrom<Vec<Vec<String>>> for Conditions {
    type Error = Error;
    fn try_from(tags: Vec<Vec<String>>) -> Result<Conditions, Self::Error> {
        let mut output = None;

        for tag in tags {
            if tag.len() < 2 {
                continue;
            }

            let tag_kind = TagKind::from(&tag[0]);
            match tag_kind {
                TagKind::Custom(ref kind) if kind == "program_output" => {
                    output = Some(Felt::from_str(&tag[1]).map_err(|e| Error::FeltFromStr(e))?);
                }
                _ => {}
            }
        }

        Ok(Conditions { output })
    }
}

/// Cairo Witness
#[derive(Default, Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct CairoWitness {
    /// The serialized .json Cairo proof
    pub cairo_proof_json: String,
}

// TODO: won't be needed once we update to the latest version of stwo_cairo_prover
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

/// Convert a PubMemoryValue to a Felt
fn pmv_to_felt(pmv: &PubMemoryValue) -> Felt {
    let (_id, value) = pmv;
    let mut le_bytes = [0u8; 32];
    for (i, &v) in value.iter().enumerate() {
        let start = i * 4;
        le_bytes[start..start + 4].copy_from_slice(&v.to_le_bytes());
    }
    Felt::from_bytes_le(&le_bytes)
}

/// Hash an array of PubMemoryValues using Poseidon
pub fn hash_array_pmv(values: &Vec<PubMemoryValue>) -> Felt {
    Poseidon::hash_array(&values.iter().map(|v| pmv_to_felt(v)).collect::<Vec<_>>())
}

impl Proof {
    /// Verify Cairo
    pub fn verify_cairo(&self) -> Result<(), Error> {
        let secret: Nut10Secret = self.secret.clone().try_into()?;
        if secret.kind().ne(&super::Kind::Cairo) {
            return Err(Error::IncorrectSecretKind);
        }

        let cairo_witness = match &self.witness {
            Some(Witness::CairoWitness(witness)) => witness,
            _ => return Err(Error::IncorrectWitnessKind),
        };
        let cairo_proof = match serde_json::from_str::<CairoProof<Blake2sMerkleHasher>>(
            &cairo_witness.cairo_proof_json,
        ) {
            Ok(proof) => proof,
            Err(e) => return Err(Error::Serde(e)),
        };

        let program_hash_condition =
            Felt::from_str(secret.secret_data().data()).map_err(|e| Error::FeltFromStr(e))?;

        let program: &Vec<PubMemoryValue> = &cairo_proof.claim.public_data.public_memory.program;
        let program_hash = hash_array_pmv(program);

        if program_hash != program_hash_condition {
            return Err(Error::ProgramHashVerification(
                program_hash,
                program_hash_condition,
            ));
        }

        let conditions: Option<Conditions> = secret
            .secret_data()
            .tags()
            .and_then(|c| c.clone().try_into().ok());

        if let Some(output_condition) = conditions.and_then(|c| c.output) {
            // check if the output in the claim matches the output in the conditions
            let output = hash_array_pmv(&cairo_proof.claim.public_data.public_memory.output);
            if output != output_condition {
                return Err(Error::OutputHashVerification(output, output_condition));
            }
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

    /// Add cairo proof
    #[inline]
    pub fn add_cairo_proof(&mut self, cairo_proof_json: String) {
        self.witness = Some(Witness::CairoWitness(CairoWitness { cairo_proof_json }))
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
    use crate::{Amount, Id, Kind, Nut10Secret, SecretKey};

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
                // This is a hack because `Felt::from_hex` doesn't work with negative numbers.
                // This is ok because we only need to parse executables during testing and thus
                // using cairo_lang_executable is not worth having an extra dependency
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
        let secret_key =
            SecretKey::from_str("99590802251e78ee1051648439eedb003dc539093a48a44e7b8f2642c909ea37")
                .unwrap();
        let v_key = secret_key.public_key();

        // Hash the program bytecode
        let executable_json = include_str!("./test/is_prime_executable.json");
        let executable: Executable = serde_json::from_str(executable_json).unwrap();
        let program_hash = Poseidon::hash_array(&executable.program.bytecode);

        // Specify output condition
        let output_false = Poseidon::hash_array(&vec![Felt::from(0)]); // is not prime
        let output_true = Poseidon::hash_array(&vec![Felt::from(1)]); // is prime

        let cond_false = Conditions {
            output: Some(output_false),
        };
        let cond_true = Conditions {
            output: Some(output_true),
        };

        let secret_is_prime_true: Secret =
            Nut10Secret::new(Kind::Cairo, program_hash.to_string(), Some(cond_true))
                .try_into()
                .unwrap();
        let secret_is_prime_false: Secret =
            Nut10Secret::new(Kind::Cairo, program_hash.to_string(), Some(cond_false))
                .try_into()
                .unwrap();

        let cairo_proof_is_prime_7 = include_str!("./test/is_prime_proof_7.json").to_string();
        let witness_is_prime_7 = CairoWitness {
            cairo_proof_json: cairo_proof_is_prime_7,
        };
        let cairo_proof_is_prime_9 = include_str!("./test/is_prime_proof_9.json").to_string();
        let witness_is_prime_9 = CairoWitness {
            cairo_proof_json: cairo_proof_is_prime_9,
        };

        // proof that is_prime(7) == true
        let mut proof: Proof = Proof {
            amount: Amount::ZERO,                                 // unused in this test
            keyset_id: Id::from_str("009a1f293253e41e").unwrap(), // unused in this test
            secret: secret_is_prime_true.clone(),
            c: v_key, // unused in this test
            witness: Some(Witness::CairoWitness(witness_is_prime_7)),
            dleq: None, // unused in this test
        };
        proof.verify_cairo().unwrap();
        assert!(proof.verify_cairo().is_ok());

        // if we change the output condition to false, the verification should fail
        proof.secret = secret_is_prime_false.clone();
        assert!(proof.verify_cairo().is_err());

        // if we change the witness to the computation of is_prime(9), it now succeeds
        proof.witness = Some(Witness::CairoWitness(witness_is_prime_9));
        assert!(proof.verify_cairo().is_ok());

        // if we change the output condition to true, the verification should again fail
        proof.secret = secret_is_prime_true.clone();
        assert!(proof.verify_cairo().is_err());
    }

    #[test]
    fn test_secret_ser() {
        // Testing the serde serialization of the secret

        // with hex string data
        let conditions = Conditions { output: None };
        let data = Felt::from_hex("0x1234567890abcdef").unwrap();
        let secret = Nut10Secret::new(Kind::Cairo, data.to_hex_string(), Some(conditions));
        let secret_str = serde_json::to_string(&secret).unwrap();
        let secret_der: Nut10Secret = serde_json::from_str(&secret_str).unwrap();

        assert_eq!(secret, secret_der);

        // with decimal string data
        let conditions = Conditions { output: None };
        let data = Felt::from_str("99").unwrap();
        let secret = Nut10Secret::new(Kind::Cairo, data.to_string(), Some(conditions));
        let secret_str = serde_json::to_string(&secret).unwrap();
        let secret_der: Nut10Secret = serde_json::from_str(&secret_str).unwrap();

        assert_eq!(secret, secret_der);
    }
}

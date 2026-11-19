//! Swap saga types

use core::fmt;

use cashu::nuts::nut00::SpendInfo;
use cashu::secret::Secret;
use cashu::{BlindedMessage, BlindingFactor, ProofId};
use serde::{Deserialize, Serialize};

use crate::{Amount, Error};

/// States specific to swap saga (wallet-side)
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwapSagaState {
    /// Input proofs reserved, swap request prepared, ready to execute
    ProofsReserved,
    /// Swap request sent to mint, awaiting signatures for new proofs
    SwapRequested,
}

impl std::fmt::Display for SwapSagaState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SwapSagaState::ProofsReserved => write!(f, "proofs_reserved"),
            SwapSagaState::SwapRequested => write!(f, "swap_requested"),
        }
    }
}

impl std::str::FromStr for SwapSagaState {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "proofs_reserved" => Ok(SwapSagaState::ProofsReserved),
            "swap_requested" => Ok(SwapSagaState::SwapRequested),
            _ => Err(Error::InvalidOperationState),
        }
    }
}

/// Operation-specific data for Swap operations
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapOperationData {
    /// Input amount
    pub input_amount: Amount,
    /// Output amount
    pub output_amount: Amount,
    /// Derivation counter start
    pub counter_start: Option<u32>,
    /// Derivation counter end
    pub counter_end: Option<u32>,
    /// Blinded messages for recovery
    ///
    /// Stored so that if a crash occurs after the mint accepts the swap,
    /// we can use these to query the mint for signatures and reconstruct proofs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blinded_messages: Option<Vec<BlindedMessage>>,
    /// Pending output material, retained until the resulting proofs are stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_outputs: Option<Vec<SwapOutput>>,
    /// Inputs to this swap, including proofs reserved by a parent operation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_ys: Vec<ProofId>,
}

/// Material needed to construct a pending swap output's proof.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct SwapOutput {
    /// Requested blinded output.
    pub blinded_message: BlindedMessage,
    /// Output secret.
    pub secret: Secret,
    /// Output blinding factor.
    pub r: BlindingFactor,
    /// Deterministic derivation index, if applicable.
    pub derivation_index: Option<u32>,
    /// Nutroot spend information to include with the output proof.
    pub spend_info: Option<SpendInfo>,
}

impl<'de> Deserialize<'de> for SwapOutput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct StoredOutput {
            blinded_message: BlindedMessage,
            secret: Secret,
            r: BlindingFactor,
            derivation_index: Option<u32>,
            spend_info: Option<SpendInfo>,
        }
        let stored = StoredOutput::deserialize(deserializer)?;
        let r = match stored.blinded_message.keyset_id.get_version() {
            cashu::KeySetVersion::Version02 => {
                BlindingFactor::bls_from_slice(&stored.r.to_secret_bytes())
                    .map_err(serde::de::Error::custom)?
            }
            _ => stored.r,
        };
        Ok(Self {
            blinded_message: stored.blinded_message,
            secret: stored.secret,
            r,
            derivation_index: stored.derivation_index,
            spend_info: stored.spend_info,
        })
    }
}

impl fmt::Debug for SwapOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SwapOutput")
            .field("blinded_message", &self.blinded_message)
            .field("secret", &"[redacted]")
            .field("r", &"[redacted]")
            .field("derivation_index", &self.derivation_index)
            .field("spend_info", &"[redacted]")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use cashu::nut01::BlsG1PublicKey;
    use cashu::{Id, SecretKey};

    use super::*;

    #[test]
    fn pending_outputs_preserve_scalar_curve_and_redact_secrets() {
        let id = Id::from_str(&format!("02{}", "00".repeat(32))).unwrap();
        let output = SwapOutput {
            blinded_message: BlindedMessage::new(
                Amount::from(1),
                id,
                BlsG1PublicKey::hash_to_curve(b"pending output"),
            ),
            secret: Secret::from_point(&SecretKey::generate().public_key()),
            r: BlindingFactor::bls_from_reduced_bytes(&[9; 32]).unwrap(),
            derivation_index: None,
            spend_info: None,
        };
        let encoded = serde_json::to_string(&output).unwrap();
        let decoded: SwapOutput = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, output);
        let debug = format!("{decoded:?}");
        assert!(!debug.contains(&output.secret.to_string()));
        assert!(!debug.contains(&cashu::util::hex::encode(output.r.to_secret_bytes())));
    }

    #[test]
    fn old_swap_records_do_not_require_pending_outputs() {
        let data: SwapOperationData = serde_json::from_value(serde_json::json!({
            "input_amount": 8, "output_amount": 8,
            "counter_start": 0, "counter_end": 1,
            "blinded_messages": null,
        }))
        .unwrap();
        assert!(data.pending_outputs.is_none());
        assert!(data.input_ys.is_empty());
    }
}

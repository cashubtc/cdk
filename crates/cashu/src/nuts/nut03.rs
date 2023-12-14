//! Request mint
// https://github.com/cashubtc/nuts/blob/main/03.md

use serde::{Deserialize, Serialize};

use super::nut00::BlindedSignature;
#[cfg(feature = "wallet")]
use crate::nuts::PreMintSecrets;
use crate::nuts::{BlindedMessage, Proofs};
use crate::Amount;
pub use crate::Bolt11Invoice;

#[cfg(feature = "wallet")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreSplit {
    pub pre_mint_secrets: PreMintSecrets,
    pub split_request: SplitRequest,
}

/// Split Request [NUT-06]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitRequest {
    /// Proofs that are to be spent in `Split`
    pub inputs: Proofs,
    /// Blinded Messages for Mint to sign
    pub outputs: Vec<BlindedMessage>,
}

impl SplitRequest {
    pub fn new(inputs: Proofs, outputs: Vec<BlindedMessage>) -> Self {
        Self { inputs, outputs }
    }

    /// Total value of proofs in `SplitRequest`
    pub fn input_amount(&self) -> Amount {
        self.inputs.iter().map(|proof| proof.amount).sum()
    }

    /// Total value of outputs in `SplitRequest`
    pub fn output_amount(&self) -> Amount {
        self.outputs.iter().map(|proof| proof.amount).sum()
    }
}

/// Split Response [NUT-06]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitResponse {
    /// Promises
    pub promises: Option<Vec<BlindedSignature>>,
}

impl SplitResponse {
    pub fn new(promises: Vec<BlindedSignature>) -> SplitResponse {
        SplitResponse {
            promises: Some(promises),
        }
    }

    pub fn promises_amount(&self) -> Option<Amount> {
        self.promises.as_ref().map(|promises| {
            promises
                .iter()
                .map(|BlindedSignature { amount, .. }| *amount)
                .sum()
        })
    }
}

//! NUT-03: Swap
//!
//! <https://github.com/cashubtc/nuts/blob/main/03.md>

use serde::{Deserialize, Serialize};

use super::nut00::BlindSignature;
#[cfg(feature = "wallet")]
use crate::nuts::PreMintSecrets;
use crate::nuts::{BlindedMessage, Proofs};
use crate::Amount;
pub use crate::Bolt11Invoice;

#[cfg(feature = "wallet")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreSwap {
    pub pre_mint_secrets: PreMintSecrets,
    pub swap_request: SwapRequest,
}

/// Split Request [NUT-06]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapRequest {
    /// Proofs that are to be spent in `Split`
    pub inputs: Proofs,
    /// Blinded Messages for Mint to sign
    pub outputs: Vec<BlindedMessage>,
}

impl SwapRequest {
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
pub struct SwapResponse {
    /// Promises
    pub signatures: Vec<BlindSignature>,
}

impl SwapResponse {
    pub fn new(promises: Vec<BlindSignature>) -> SwapResponse {
        SwapResponse {
            signatures: promises,
        }
    }

    pub fn promises_amount(&self) -> Amount {
        self.signatures
            .iter()
            .map(|BlindSignature { amount, .. }| *amount)
            .sum()
    }
}

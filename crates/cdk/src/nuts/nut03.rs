//! NUT-03: Swap
//!
//! <https://github.com/cashubtc/nuts/blob/main/03.md>

use serde::{Deserialize, Serialize};

use super::nut00::{BlindSignature, BlindedMessage, PreMintSecrets, Proofs};
use crate::Amount;

/// Preswap information
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreSwap {
    /// Preswap mint secrets
    pub pre_mint_secrets: PreMintSecrets,
    /// Swap request
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
    /// Create new [`SwapRequest`]
    pub fn new(inputs: Proofs, outputs: Vec<BlindedMessage>) -> Self {
        Self { inputs, outputs }
    }

    /// Total value of proofs in [`SwapRequest`]
    pub fn input_amount(&self) -> Amount {
        self.inputs.iter().map(|proof| proof.amount).sum()
    }

    /// Total value of outputs in [`SwapRequest`]
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
    /// Create new [`SwapRequest`]
    pub fn new(promises: Vec<BlindSignature>) -> SwapResponse {
        SwapResponse {
            signatures: promises,
        }
    }

    /// Total [`Amount`] of promises
    pub fn promises_amount(&self) -> Amount {
        self.signatures
            .iter()
            .map(|BlindSignature { amount, .. }| *amount)
            .sum()
    }
}

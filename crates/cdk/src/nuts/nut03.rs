//! NUT-03: Swap
//!
//! <https://github.com/cashubtc/nuts/blob/main/03.md>

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::nut00::{BlindSignature, BlindedMessage, PreMintSecrets, Proofs};
use crate::Amount;

/// NUT03 Error
#[derive(Debug, Error)]
pub enum Error {
    /// DHKE error
    #[error(transparent)]
    DHKE(#[from] crate::dhke::Error),
    /// Amount Error
    #[error(transparent)]
    Amount(#[from] crate::amount::Error),
}

/// Preswap information
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreSwap {
    /// Preswap mint secrets
    pub pre_mint_secrets: PreMintSecrets,
    /// Swap request
    pub swap_request: SwapRequest,
    /// Amount to increment keyset counter by
    pub derived_secret_count: u32,
    /// Fee amount
    pub fee: Amount,
}

/// Swap Request [NUT-03]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct SwapRequest {
    /// Proofs that are to be spent in a `Swap`
    #[cfg_attr(feature = "swagger", schema(value_type = Vec<Proof>))]
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
    pub fn input_amount(&self) -> Result<Amount, Error> {
        Ok(Amount::try_sum(
            self.inputs.iter().map(|proof| proof.amount),
        )?)
    }

    /// Total value of outputs in [`SwapRequest`]
    pub fn output_amount(&self) -> Result<Amount, Error> {
        Ok(Amount::try_sum(
            self.outputs.iter().map(|proof| proof.amount),
        )?)
    }
}

/// Split Response [NUT-06]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
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
    pub fn promises_amount(&self) -> Result<Amount, Error> {
        Ok(Amount::try_sum(
            self.signatures
                .iter()
                .map(|BlindSignature { amount, .. }| *amount),
        )?)
    }
}

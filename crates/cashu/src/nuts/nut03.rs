//! NUT-03: Swap
//!
//! <https://github.com/cashubtc/nuts/blob/main/03.md>

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[cfg(feature = "wallet")]
use super::nut00::PreMintSecrets;
use super::nut00::{BlindSignature, BlindedMessage, Proofs};
use super::ProofsMethods;
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
#[cfg(feature = "wallet")]
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
    #[cfg_attr(feature = "swagger", schema(value_type = Vec<crate::Proof>))]
    inputs: Proofs,
    /// Blinded Messages for Mint to sign
    outputs: Vec<BlindedMessage>,
}

impl SwapRequest {
    /// Create new [`SwapRequest`]
    pub fn new(inputs: Proofs, outputs: Vec<BlindedMessage>) -> Self {
        Self {
            inputs: inputs.without_dleqs(),
            outputs,
        }
    }

    /// Get inputs (proofs)
    pub fn inputs(&self) -> &Proofs {
        &self.inputs
    }

    /// Get mutable inputs (proofs)
    pub fn inputs_mut(&mut self) -> &mut Proofs {
        &mut self.inputs
    }

    /// Get outputs (blinded messages)
    pub fn outputs(&self) -> &Vec<BlindedMessage> {
        &self.outputs
    }

    /// Get mutable reference to outputs (blinded messages)
    pub fn outputs_mut(&mut self) -> &mut Vec<BlindedMessage> {
        &mut self.outputs
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

impl super::nut10::SpendingConditionVerification for SwapRequest {
    fn inputs(&self) -> &Proofs {
        &self.inputs
    }

    fn sig_all_msg_to_sign(&self) -> String {
        let mut msg = String::new();

        // Add all input secrets and C values in order
        // msg = secret_0 || C_0 || ... || secret_n || C_n
        for proof in &self.inputs {
            msg.push_str(&proof.secret.to_string());
            msg.push_str(&proof.c.to_hex());
        }

        // Add all output amounts and B_ values in order
        // msg = ... || amount_0 || B_0 || ... || amount_m || B_m
        for output in &self.outputs {
            msg.push_str(&output.amount.to_string());
            msg.push_str(&output.blinded_secret.to_hex());
        }

        msg
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
    /// Create new [`SwapResponse`]
    pub fn new(promises: Vec<BlindSignature>) -> Self {
        Self {
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

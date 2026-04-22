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
    /// Ephemeral secret keys used for p2bk
    pub p2bk_secret_keys: Option<Vec<crate::nuts::nut01::SecretKey>>,
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

#[cfg(test)]
mod tests {
    use super::*;

    const SWAP_REQUEST_JSON: &str = r#"{
        "inputs": [
            {
                "amount": 2,
                "id": "00bfa73302d12ffd",
                "secret": "[\"P2PK\",{\"nonce\":\"c7f280eb55c1e8564e03db06973e94bc9b666d9e1ca42ad278408fe625950303\",\"data\":\"030d8acedfe072c9fa449a1efe0817157403fbec460d8e79f957966056e5dd76c1\",\"tags\":[[\"sigflag\",\"SIG_ALL\"]]}]",
                "C": "02c97ee3d1db41cf0a3ddb601724be8711a032950811bf326f8219c50c4808d3cd",
                "witness": "{\"signatures\":[\"ce017ca25b1b97df2f72e4b49f69ac26a240ce14b3690a8fe619d41ccc42d3c1282e073f85acd36dc50011638906f35b56615f24e4d03e8effe8257f6a808538\"]}"
            },
            {
                "amount": 4,
                "id": "00bfa73302d12ffd",
                "secret": "[\"P2PK\",{\"nonce\":\"d7f280eb55c1e8564e03db06973e94bc9b666d9e1ca42ad278408fe625950304\",\"data\":\"030d8acedfe072c9fa449a1efe0817157403fbec460d8e79f957966056e5dd76c1\",\"tags\":[[\"sigflag\",\"SIG_ALL\"]]}]",
                "C": "02c97ee3d1db41cf0a3ddb601724be8711a032950811bf326f8219c50c4808d3cd"
            }
        ],
        "outputs": [
            {
                "amount": 2,
                "id": "00bfa73302d12ffd",
                "B_": "038ec853d65ae1b79b5cdbc2774150b2cb288d6d26e12958a16fb33c32d9a86c39"
            }
        ]
    }"#;

    #[test]
    fn test_swap_request_inputs_outputs_getters() {
        // Kills mutations that replace `inputs()` / `outputs()` with empty
        // leaked boxes by asserting both length and element content.
        let req: SwapRequest = serde_json::from_str(SWAP_REQUEST_JSON).unwrap();

        let inputs = req.inputs();
        assert_eq!(inputs.len(), 2, "expected 2 inputs");
        assert_eq!(u64::from(inputs[0].amount), 2);
        assert_eq!(u64::from(inputs[1].amount), 4);

        let outputs = req.outputs();
        assert_eq!(outputs.len(), 1, "expected 1 output");
        assert_eq!(u64::from(outputs[0].amount), 2);
    }

    #[test]
    fn test_swap_request_inputs_outputs_getters_via_new() {
        // Round-trip through SwapRequest::new to ensure the getters
        // return the data we constructed the request with, not an empty default.
        let req: SwapRequest = serde_json::from_str(SWAP_REQUEST_JSON).unwrap();
        let inputs_clone = req.inputs().clone();
        let outputs_clone = req.outputs().clone();

        let rebuilt = SwapRequest::new(inputs_clone, outputs_clone);
        assert_eq!(rebuilt.inputs().len(), 2);
        assert_eq!(rebuilt.outputs().len(), 1);
        assert!(!rebuilt.inputs().is_empty());
        assert!(!rebuilt.outputs().is_empty());
    }
}

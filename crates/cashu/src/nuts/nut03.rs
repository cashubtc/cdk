//! NUT-03: Swap
//!
//! <https://github.com/cashubtc/nuts/blob/main/03.md>

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::{Scalar, SecretKey};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[cfg(feature = "wallet")]
use super::nut00::PreMintSecrets;
use super::nut00::{BlindSignature, BlindedMessage, Proofs};
use super::nut01::PublicKey;
use super::ProofsMethods;
use crate::Amount;
use crate::SECP256K1;

const UNPOISONABLE_TWEAK_DOMAIN: &[u8] = b"Cashu/Unpoisonable/secp256k1/v1\0";

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

/// Derive a nonzero secp256k1 scalar from the mint's 32-byte random value and an original point.
///
/// Rejection sampling uses a big-endian counter, starting at zero, until the SHA-256
/// digest represents an integer in `[1, n-1]`. Points use compressed SEC1 encoding.
pub fn derive_unpoisonable_tweak(r: &[u8; 32], original: &PublicKey) -> Result<Scalar, Error> {
    for counter in 0..=u32::MAX {
        let mut preimage = Vec::with_capacity(UNPOISONABLE_TWEAK_DOMAIN.len() + 32 + 33 + 4);
        preimage.extend_from_slice(UNPOISONABLE_TWEAK_DOMAIN);
        preimage.extend_from_slice(r);
        preimage.extend_from_slice(&original.to_bytes());
        preimage.extend_from_slice(&counter.to_be_bytes());
        let digest = Sha256Hash::hash(&preimage).to_byte_array();
        if let Ok(secret) = SecretKey::from_slice(&digest) {
            return Ok(Scalar::from(secret));
        }
    }
    Err(crate::dhke::Error::Custom("Could not derive a nonzero tweak".to_owned()).into())
}

/// Derive a tweaked blinded point using the same `R` as the other outputs in a swap.
pub fn tweak_blinded_point(r: &[u8; 32], original: &PublicKey) -> Result<PublicKey, Error> {
    let tweak = derive_unpoisonable_tweak(r, original)?;
    Ok(original
        .mul_tweak(&SECP256K1, &tweak)
        .map_err(crate::dhke::Error::from)?
        .into())
}

/// Build the execution outputs for an unpoisonable swap, retaining their original order.
///
/// Callers must check that the original points are distinct before deriving outputs.
/// A collision between tweaked points requires choosing a new `R`.
pub fn tweak_swap_outputs(
    r: &[u8; 32],
    original: &[BlindedMessage],
) -> Result<Vec<BlindedMessage>, Error> {
    original
        .iter()
        .map(|output| {
            let point = tweak_blinded_point(r, &output.blinded_secret)?;
            let mut output = output.clone();
            output.blinded_secret = point;
            Ok(output)
        })
        .collect()
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
pub struct SwapRequest {
    /// Proofs that are to be spent in a `Swap`
    inputs: Proofs,
    /// Blinded Messages for Mint to sign
    outputs: Vec<BlindedMessage>,
    /// Whether this swap requires unpoisonable output processing.
    #[serde(default, skip_serializing_if = "is_false")]
    unpoisonable: bool,
}

fn is_false(value: &bool) -> bool {
    !value
}

impl SwapRequest {
    /// Create new [`SwapRequest`]
    pub fn new(inputs: Proofs, outputs: Vec<BlindedMessage>) -> Self {
        Self {
            inputs: inputs.without_dleqs(),
            outputs,
            unpoisonable: false,
        }
    }

    /// Select unpoisonable output processing for this swap.
    pub fn with_unpoisonable(mut self) -> Self {
        self.unpoisonable = true;
        self
    }

    /// Whether this swap requires unpoisonable output processing.
    pub fn is_unpoisonable(&self) -> bool {
        self.unpoisonable
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
    fn unpoisonable_sig_all_msg_to_sign(&self) -> Option<Vec<u8>> {
        self.unpoisonable
            .then(|| super::nut10::unpoisonable_sig_all_msg_to_sign(&self.inputs, &self.outputs))
    }
}

/// Split Response [NUT-06]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

    #[test]
    fn test_swap_request_outputs_mut_updates_outputs() {
        let mut req: SwapRequest = serde_json::from_str(SWAP_REQUEST_JSON).unwrap();
        let output = req.outputs()[0].clone();

        req.outputs_mut().push(output);

        assert_eq!(req.outputs().len(), 2);
    }

    #[test]
    fn test_swap_request_amounts() {
        let req: SwapRequest = serde_json::from_str(SWAP_REQUEST_JSON).unwrap();

        assert_eq!(req.input_amount().unwrap(), Amount::from(6));
        assert_eq!(req.output_amount().unwrap(), Amount::from(2));
    }

    #[test]
    fn test_swap_response_promises_amount() {
        let response: SwapResponse = serde_json::from_str(
            r#"{
                "signatures": [
                    {
                        "amount": 8,
                        "id": "00bfa73302d12ffd",
                        "C_": "02c97ee3d1db41cf0a3ddb601724be8711a032950811bf326f8219c50c4808d3cd"
                    },
                    {
                        "amount": 4,
                        "id": "00bfa73302d12ffd",
                        "C_": "038ec853d65ae1b79b5cdbc2774150b2cb288d6d26e12958a16fb33c32d9a86c39"
                    }
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(response.promises_amount().unwrap(), Amount::from(12));
    }

    #[test]
    fn unpoisonable_tweak_vector_and_signing_linearity() {
        let r = [0x42; 32];
        let original = PublicKey::from_hex(
            "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
        )
        .unwrap();
        let scalar = derive_unpoisonable_tweak(&r, &original).unwrap();
        assert_eq!(
            scalar.to_be_bytes(),
            crate::util::hex::decode(
                "b99dcc04615c5c74c8797e18e4e3c38d915f06677ba60c95fec1af2e105aeeea"
            )
            .unwrap()
            .as_slice()
        );

        let mint_key = crate::nuts::nut01::SecretKey::generate();
        let signed_tweaked =
            crate::dhke::sign_message(&mint_key, &tweak_blinded_point(&r, &original).unwrap())
                .unwrap();
        let signed_original = crate::dhke::sign_message(&mint_key, &original).unwrap();
        assert_eq!(
            signed_tweaked,
            PublicKey::from(signed_original.mul_tweak(&SECP256K1, &scalar).unwrap())
        );
        assert_ne!(
            tweak_blinded_point(&[0x43; 32], &original).unwrap(),
            tweak_blinded_point(&r, &original).unwrap()
        );
    }

    #[test]
    fn unpoisonable_outputs_preserve_order() {
        let request: SwapRequest = serde_json::from_str(SWAP_REQUEST_JSON).unwrap();
        let first = request.outputs()[0].clone();
        let second = BlindedMessage::new(
            Amount::from(4),
            first.keyset_id,
            crate::nuts::nut01::SecretKey::generate().public_key(),
        );
        let r = [0x42; 32];
        let outputs = tweak_swap_outputs(&r, &[first.clone(), second.clone()]).unwrap();
        assert_eq!(outputs[0].amount, first.amount);
        assert_eq!(outputs[1].amount, second.amount);
        assert_eq!(
            outputs[0].blinded_secret,
            tweak_blinded_point(&r, &first.blinded_secret).unwrap()
        );
        assert_eq!(
            outputs[1].blinded_secret,
            tweak_blinded_point(&r, &second.blinded_secret).unwrap()
        );
    }
}

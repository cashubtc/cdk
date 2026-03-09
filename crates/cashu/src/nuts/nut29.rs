//! NUT-29: Batch Mint Tokens
//!
//! <https://github.com/cashubtc/nuts/blob/main/29.md>

use bitcoin::secp256k1::schnorr::Signature;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::nut01::Error as Nut01Error;
use super::{PublicKey, SecretKey};
use crate::{Amount, BlindedMessage};

/// Error types for batch operations
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Signature error from nut20
    #[error(transparent)]
    Signature(#[from] super::nut20::Error),
    /// NUT-01 error
    #[error(transparent)]
    Nut01(#[from] Nut01Error),
}

/// NUT-29 Settings for mint info
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct Settings {
    /// Maximum number of quotes allowed in a single batch request
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_batch_size: Option<u64>,
    /// Supported payment methods for batch minting
    #[serde(skip_serializing_if = "Option::is_none")]
    pub methods: Option<Vec<String>>,
}

impl Settings {
    /// Create new NUT-29 settings
    pub fn new(max_batch_size: Option<u64>, methods: Option<Vec<String>>) -> Self {
        Self {
            max_batch_size,
            methods,
        }
    }

    /// Check if settings are empty (no configuration)
    pub fn is_empty(&self) -> bool {
        self.max_batch_size.is_none() && self.methods.is_none()
    }
}

/// Batch check mint quote request per NUT-29
///
/// Used to check the state of multiple mint quotes at once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound = "Q: Serialize + DeserializeOwned")]
pub struct BatchCheckMintQuoteRequest<Q> {
    /// Array of unique mint quote IDs to check
    pub quotes: Vec<Q>,
}

/// Batch mint request per NUT-29
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound = "Q: Serialize + DeserializeOwned")]
pub struct BatchMintRequest<Q> {
    /// Array of unique quote IDs
    pub quotes: Vec<Q>,
    /// Optional expected amounts per quote (for bolt12-like methods)
    pub quote_amounts: Option<Vec<Amount>>,
    /// Shared outputs across all quotes
    pub outputs: Vec<BlindedMessage>,
    /// Signatures per quote (None if all quotes are unlocked)
    pub signatures: Option<Vec<Option<String>>>,
}

impl<Q> BatchMintRequest<Q>
where
    Q: ToString,
{
    /// Constructs the message to be signed according to NUT-20 for a quote.
    ///
    /// The message is constructed by concatenating (as UTF-8 encoded bytes):
    /// 1. The quote ID (as UTF-8)
    /// 2. All blinded secrets (B_0 through B_n) converted to hex strings (as UTF-8)
    ///
    /// Format: `quote_id || B_0 || B_1 || ... || B_n`
    /// where each component is encoded as UTF-8 bytes.
    pub fn msg_to_sign(&self, quote: &Q) -> Vec<u8> {
        let quote_id = quote.to_string();
        let capacity = quote_id.len() + (self.outputs.len() * 66);
        let mut msg = Vec::with_capacity(capacity);

        msg.extend_from_slice(quote_id.as_bytes());

        for output in &self.outputs {
            msg.extend_from_slice(output.blinded_secret.to_hex().as_bytes());
        }

        msg
    }

    /// Sign one quote inside a batch mint request.
    pub fn sign_quote(&self, quote: &Q, secret_key: &SecretKey) -> Result<String, Error> {
        let msg = self.msg_to_sign(quote);
        let signature: Signature = secret_key.sign(&msg)?;
        Ok(signature.to_string())
    }

    /// Verify one quote signature inside a batch mint request.
    pub fn verify_quote_signature(
        &self,
        quote: &Q,
        signature: &str,
        pubkey: &PublicKey,
    ) -> Result<(), Error> {
        let signature = signature
            .parse::<Signature>()
            .map_err(|_| super::nut20::Error::InvalidSignature)?;
        let msg = self.msg_to_sign(quote);

        pubkey
            .verify(&msg, &signature)
            .map_err(|_| super::nut20::Error::InvalidSignature)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::Id;

    fn dummy_blinded_message() -> BlindedMessage {
        let secret_key = SecretKey::generate();
        BlindedMessage {
            amount: Amount::from(1),
            keyset_id: Id::from_str("009a1f293253e41e").unwrap(),
            blinded_secret: secret_key.public_key(),
            witness: None,
        }
    }

    #[test]
    fn test_sign_and_verify_batch_quote_roundtrip() {
        let secret_key = SecretKey::generate();
        let pubkey = secret_key.public_key();
        let quote_id = "test-quote-id-123".to_string();
        let outputs = vec![dummy_blinded_message(), dummy_blinded_message()];
        let request = BatchMintRequest {
            quotes: vec![quote_id.clone()],
            quote_amounts: None,
            outputs,
            signatures: None,
        };

        let signature = request
            .sign_quote(&quote_id, &secret_key)
            .expect("signing should succeed");

        request
            .verify_quote_signature(&quote_id, &signature, &pubkey)
            .expect("verification should succeed with correct key");
    }

    #[test]
    fn test_verify_batch_quote_wrong_key() {
        let signing_key = SecretKey::generate();
        let wrong_key = SecretKey::generate();
        let quote_id = "test-quote-wrong-key".to_string();
        let outputs = vec![dummy_blinded_message()];
        let request = BatchMintRequest {
            quotes: vec![quote_id.clone()],
            quote_amounts: None,
            outputs,
            signatures: None,
        };

        let signature = request
            .sign_quote(&quote_id, &signing_key)
            .expect("signing should succeed");

        let result = request.verify_quote_signature(&quote_id, &signature, &wrong_key.public_key());
        assert!(result.is_err(), "verification should fail with wrong key");
    }

    #[test]
    fn test_verify_batch_quote_tampered_outputs() {
        let secret_key = SecretKey::generate();
        let pubkey = secret_key.public_key();
        let quote_id = "test-quote-tampered".to_string();
        let outputs = vec![dummy_blinded_message(), dummy_blinded_message()];
        let request = BatchMintRequest {
            quotes: vec![quote_id.clone()],
            quote_amounts: None,
            outputs,
            signatures: None,
        };

        let signature = request
            .sign_quote(&quote_id, &secret_key)
            .expect("signing should succeed");

        let tampered_request = BatchMintRequest {
            quotes: vec![quote_id.clone()],
            quote_amounts: None,
            outputs: vec![dummy_blinded_message(), dummy_blinded_message()],
            signatures: None,
        };

        let result = tampered_request.verify_quote_signature(&quote_id, &signature, &pubkey);
        assert!(
            result.is_err(),
            "verification should fail with tampered outputs"
        );
    }

    #[test]
    fn test_verify_batch_quote_wrong_quote_id() {
        let secret_key = SecretKey::generate();
        let pubkey = secret_key.public_key();
        let quote_id = "original-quote-id".to_string();
        let outputs = vec![dummy_blinded_message()];
        let request = BatchMintRequest {
            quotes: vec![quote_id.clone()],
            quote_amounts: None,
            outputs,
            signatures: None,
        };

        let signature = request
            .sign_quote(&quote_id, &secret_key)
            .expect("signing should succeed");

        let different_quote_id = "different-quote-id".to_string();
        let result = request.verify_quote_signature(&different_quote_id, &signature, &pubkey);
        assert!(
            result.is_err(),
            "verification should fail with different quote ID"
        );
    }

    #[test]
    fn test_sign_batch_quote_empty_outputs() {
        let secret_key = SecretKey::generate();
        let pubkey = secret_key.public_key();
        let quote_id = "test-empty-outputs".to_string();
        let request = BatchMintRequest {
            quotes: vec![quote_id.clone()],
            quote_amounts: None,
            outputs: vec![],
            signatures: None,
        };

        let signature = request
            .sign_quote(&quote_id, &secret_key)
            .expect("signing should succeed");

        request
            .verify_quote_signature(&quote_id, &signature, &pubkey)
            .expect("verification should succeed even with empty outputs");
    }

    #[test]
    fn test_sign_batch_quote_multiple_quotes_different_sigs() {
        let secret_key = SecretKey::generate();
        let outputs = vec![dummy_blinded_message(), dummy_blinded_message()];
        let quote_1 = "quote-1".to_string();
        let quote_2 = "quote-2".to_string();
        let request = BatchMintRequest {
            quotes: vec![quote_1.clone(), quote_2.clone()],
            quote_amounts: None,
            outputs,
            signatures: None,
        };

        let sig1 = request
            .sign_quote(&quote_1, &secret_key)
            .expect("signing should succeed");
        let sig2 = request
            .sign_quote(&quote_2, &secret_key)
            .expect("signing should succeed");

        assert_ne!(
            sig1, sig2,
            "signatures for different quote IDs should differ"
        );
    }
}

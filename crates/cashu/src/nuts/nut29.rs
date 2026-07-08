//! NUT-29: Batch Mint Tokens
//!
//! <https://github.com/cashubtc/nuts/blob/main/29.md>

use bitcoin::secp256k1::schnorr::Signature;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::nut01::Error as Nut01Error;
use super::nut20::{legacy_mint_quote_msg_to_sign, mint_quote_msg_to_sign};
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
    /// The message is domain-separated and length-prefixed, committing to the
    /// quote ID, output amounts, and all batch outputs in request order.
    pub fn msg_to_sign(&self, quote: &Q) -> Vec<u8> {
        let quote_id = quote.to_string();
        mint_quote_msg_to_sign(&quote_id, &self.outputs)
    }

    fn legacy_msg_to_sign(&self, quote: &Q) -> Vec<u8> {
        let quote_id = quote.to_string();
        legacy_mint_quote_msg_to_sign(&quote_id, &self.outputs)
    }

    /// Sign one quote inside a batch mint request.
    pub fn sign_quote(&self, quote: &Q, secret_key: &SecretKey) -> Result<String, Error> {
        let msg = self.msg_to_sign(quote);
        let signature: Signature = secret_key.sign(&msg)?;
        Ok(signature.to_string())
    }

    /// Sign one quote using the legacy NUT-20 message format.
    ///
    /// This is only for wallet compatibility retries against mints that have
    /// not yet upgraded to the domain-separated quote signature message.
    pub fn sign_quote_legacy(&self, quote: &Q, secret_key: &SecretKey) -> Result<String, Error> {
        let msg = self.legacy_msg_to_sign(quote);
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
        let quote_id = quote.to_string();
        let msg = mint_quote_msg_to_sign(&quote_id, &self.outputs);

        match pubkey.verify(&msg, &signature) {
            Ok(()) => return Ok(()),
            Err(_) => {
                let legacy_msg = legacy_mint_quote_msg_to_sign(&quote_id, &self.outputs);
                pubkey
                    .verify(&legacy_msg, &signature)
                    .map_err(|_| super::nut20::Error::InvalidSignature)?;
                tracing::warn!(
                    quote_id = %quote_id,
                    output_count = self.outputs.len(),
                    "Accepted legacy batch mint quote signature format"
                );
            }
        }

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

    fn blinded_message(amount: u64, blinded_secret: &str) -> BlindedMessage {
        BlindedMessage {
            amount: Amount::from(amount),
            keyset_id: Id::from_str("009a1f293253e41e").expect("valid keyset id"),
            blinded_secret: PublicKey::from_hex(blinded_secret).expect("valid blinded secret"),
            witness: None,
        }
    }

    fn test_vector_outputs() -> Vec<BlindedMessage> {
        vec![
            blinded_message(
                1,
                "036d6caac248af96f6afa7f904f550253a0f3ef3f5aa2fe6838a95b216691468e2",
            ),
            blinded_message(
                1,
                "021f8a566c205633d029094747d2e18f44e05993dda7a5f88f496078205f656e59",
            ),
        ]
    }

    #[test]
    fn test_batch_msg_to_sign_matches_nut29_vector() {
        let request = BatchMintRequest {
            quotes: vec!["locked-quote".to_string()],
            quote_amounts: None,
            outputs: test_vector_outputs(),
            signatures: None,
        };

        let msg = request.msg_to_sign(&"locked-quote".to_string());

        assert_eq!(
            crate::util::hex::encode(&msg),
            "43617368755f4d696e7451756f74655369675f76310000000c6c6f636b65642d71756f7465000000010100000021036d6caac248af96f6afa7f904f550253a0f3ef3f5aa2fe6838a95b216691468e2000000010100000021021f8a566c205633d029094747d2e18f44e05993dda7a5f88f496078205f656e59"
        );

        use bitcoin::hashes::sha256::Hash as Sha256Hash;
        use bitcoin::hashes::Hash;

        assert_eq!(
            Sha256Hash::hash(&msg).to_string(),
            "03dc68d6617bba502d8648efd0965bf393841082cf04fd03e5de4bcb5777cdfc"
        );
    }

    #[test]
    fn test_batch_signature_matches_nut29_vector() {
        let quote_id = "locked-quote".to_string();
        let pubkey = PublicKey::from_hex(
            "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
        )
        .expect("valid pubkey");
        let request = BatchMintRequest {
            quotes: vec![quote_id.clone()],
            quote_amounts: None,
            outputs: test_vector_outputs(),
            signatures: Some(vec![Some(
                "a913e48177027d87e0e38c6f2021763c46997ff4866a4b63ebca800b0776b28519eab37377cf9bc1869e489d7b25747b7a998eaa1c33c2cac7fa168449d8267a".to_string(),
            )]),
        };

        request
            .verify_quote_signature(
                &quote_id,
                "a913e48177027d87e0e38c6f2021763c46997ff4866a4b63ebca800b0776b28519eab37377cf9bc1869e489d7b25747b7a998eaa1c33c2cac7fa168449d8267a",
                &pubkey,
            )
            .expect("verification should succeed");
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
    fn test_sign_and_verify_batch_quote_legacy_roundtrip() {
        let secret_key = SecretKey::generate();
        let pubkey = secret_key.public_key();
        let quote_id = "test-quote-id-legacy".to_string();
        let outputs = vec![dummy_blinded_message(), dummy_blinded_message()];
        let request = BatchMintRequest {
            quotes: vec![quote_id.clone()],
            quote_amounts: None,
            outputs,
            signatures: None,
        };

        let signature = request
            .sign_quote_legacy(&quote_id, &secret_key)
            .expect("legacy signing should succeed");

        request
            .verify_quote_signature(&quote_id, &signature, &pubkey)
            .expect("verification should fall back to the legacy message format");
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

    #[test]
    fn settings_is_empty_requires_both_optional_fields_to_be_absent() {
        assert!(Settings::new(None, None).is_empty());
        assert!(!Settings::new(Some(10), None).is_empty());
        assert!(!Settings::new(None, Some(vec!["bolt11".to_string()])).is_empty());
        assert!(!Settings::new(Some(10), Some(vec!["bolt11".to_string()])).is_empty());
    }
}

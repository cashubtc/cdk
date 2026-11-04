//! NUT-XX: Mint Quote Lookup by Public Key
//!
//! <https://github.com/cashubtc/nuts/blob/get-quotes-by-pubkeys/xx.md>

use bitcoin::secp256k1::schnorr::Signature;
use serde::{Deserialize, Serialize};

use super::PublicKey;

/// Domain separator for mint quote lookup signatures [NUT-XX]
pub const MINT_QUOTE_LOOKUP_DOMAIN: &[u8] = b"Cashu_MintQuoteLookup_v1";

/// Maximum number of pubkeys accepted in a single lookup request.
///
/// The endpoint is unauthenticated until the signatures are checked, so the request length
/// bounds how much signature verification an anonymous caller can ask the mint to perform.
pub const MAX_LOOKUP_PUBKEYS: usize = 50;

/// Mint quote by pubkey request [NUT-XX]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MintQuoteByPubkeyRequest {
    /// NUT-20 public keys to look up quotes for
    pub pubkeys: Vec<PublicKey>,
    /// Schnorr signatures, in the same order as `pubkeys`
    pub pubkey_signatures: Vec<Signature>,
}

/// Mint quote by pubkey response [NUT-XX]
///
/// Generic over the quote representation so the mint can answer with its own response type
/// without this crate depending on the unified `MintQuoteResponse` enum in `cdk-common`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MintQuoteByPubkeyResponse<T> {
    /// Quotes locked to the requested pubkeys, in [NUT-04] response format
    pub quotes: Vec<T>,
}

/// Build the message a wallet signs to prove control of `pubkey`.
///
/// Per [NUT-XX] the signature covers the SHA-256 hash of
/// `"Cashu_MintQuoteLookup_v1" || mint_pubkey || pubkey`, with the pubkeys concatenated as
/// their UTF-8 hex string representations.
///
/// The returned value is the *pre-image*, not the digest — pass it straight to
/// [`PublicKey::verify`] / [`crate::SecretKey::sign`], both of which hash their argument.
pub fn mint_quote_lookup_msg_to_sign(mint_pubkey: &PublicKey, pubkey: &PublicKey) -> Vec<u8> {
    let mint_pubkey = mint_pubkey.to_hex();
    let pubkey = pubkey.to_hex();

    let mut msg =
        Vec::with_capacity(MINT_QUOTE_LOOKUP_DOMAIN.len() + mint_pubkey.len() + pubkey.len());
    msg.extend_from_slice(MINT_QUOTE_LOOKUP_DOMAIN);
    msg.extend_from_slice(mint_pubkey.as_bytes());
    msg.extend_from_slice(pubkey.as_bytes());
    msg
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SecretKey;

    /// Fixed keys so the vector below is reproducible.
    const MINT_SECRET: &str = "0000000000000000000000000000000000000000000000000000000000000001";
    const WALLET_SECRET: &str = "0000000000000000000000000000000000000000000000000000000000000002";

    fn fixed_keys() -> (PublicKey, PublicKey) {
        (
            SecretKey::from_hex(MINT_SECRET).unwrap().public_key(),
            SecretKey::from_hex(WALLET_SECRET).unwrap().public_key(),
        )
    }

    /// Pins the pre-image so the construction cannot drift from the NUT without this failing.
    #[test]
    fn test_msg_to_sign_vector() {
        let (mint_pubkey, pubkey) = fixed_keys();

        let msg = mint_quote_lookup_msg_to_sign(&mint_pubkey, &pubkey);

        assert_eq!(
            String::from_utf8(msg).unwrap(),
            "Cashu_MintQuoteLookup_v1\
             0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798\
             02c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5"
        );
    }

    /// A signature produced over the pre-image verifies, and one bound to a different mint
    /// does not — this is what stops a signature being replayed at another mint.
    #[test]
    fn test_sign_and_verify_is_mint_bound() {
        let (mint_pubkey, _) = fixed_keys();
        let secret_key = SecretKey::generate();
        let pubkey = secret_key.public_key();

        let msg = mint_quote_lookup_msg_to_sign(&mint_pubkey, &pubkey);
        let signature = secret_key.sign(&msg).unwrap();
        assert!(pubkey.verify(&msg, &signature).is_ok());

        let other_mint = SecretKey::generate().public_key();
        let other_msg = mint_quote_lookup_msg_to_sign(&other_mint, &pubkey);
        assert!(pubkey.verify(&other_msg, &signature).is_err());
    }

    /// A signature is bound to the pubkey it authorises, so one pubkey's signature cannot be
    /// used to read another pubkey's quotes.
    #[test]
    fn test_signature_is_bound_to_pubkey() {
        let (mint_pubkey, _) = fixed_keys();
        let secret_key = SecretKey::generate();
        let victim = SecretKey::generate().public_key();

        let msg = mint_quote_lookup_msg_to_sign(&mint_pubkey, &secret_key.public_key());
        let signature = secret_key.sign(&msg).unwrap();

        let victim_msg = mint_quote_lookup_msg_to_sign(&mint_pubkey, &victim);
        assert!(victim.verify(&victim_msg, &signature).is_err());
    }

    /// The wire format is hex strings, per the NUT.
    #[test]
    fn test_request_wire_format() {
        let (mint_pubkey, _) = fixed_keys();
        let secret_key = SecretKey::generate();
        let pubkey = secret_key.public_key();
        let msg = mint_quote_lookup_msg_to_sign(&mint_pubkey, &pubkey);
        let signature = secret_key.sign(&msg).unwrap();

        let json = serde_json::to_string(&MintQuoteByPubkeyRequest {
            pubkeys: vec![pubkey],
            pubkey_signatures: vec![signature],
        })
        .unwrap();

        assert!(json.contains(&pubkey.to_hex()));
        assert!(json.contains(&signature.to_string()));

        let request: MintQuoteByPubkeyRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(request.pubkeys, vec![pubkey]);
        assert_eq!(request.pubkey_signatures, vec![signature]);
    }

    /// The response envelope is an object with a `quotes` array, not a bare array.
    #[test]
    fn test_response_wire_format() {
        let response = MintQuoteByPubkeyResponse {
            quotes: vec![serde_json::json!({"quote": "abc", "method": "bolt11"})],
        };

        assert_eq!(
            serde_json::to_value(&response).unwrap(),
            serde_json::json!({"quotes": [{"quote": "abc", "method": "bolt11"}]})
        );
    }
}

//! NUT-XX: Mint Quote Lookup by Public Key
//!
//! <https://github.com/cashubtc/nuts/blob/get-quotes-by-pubkeys/xx.md>

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
    /// Pubkeys
    pub pubkeys: Vec<String>,
    /// Signatures
    pub pubkey_signatures: Vec<String>,
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

    /// The signed message is the pre-image, not its digest: `sign`/`verify` hash internally.
    #[test]
    fn test_signature_covers_a_single_hash() {
        use bitcoin::hashes::sha256::Hash as Sha256Hash;
        use bitcoin::hashes::Hash;

        let (mint_pubkey, _) = fixed_keys();
        let secret_key = SecretKey::generate();
        let pubkey = secret_key.public_key();

        let msg = mint_quote_lookup_msg_to_sign(&mint_pubkey, &pubkey);
        let signature = secret_key.sign(&msg).unwrap();
        assert!(pubkey.verify(&msg, &signature).is_ok());

        let digest = Sha256Hash::hash(&msg).to_byte_array();
        assert!(pubkey.verify(&digest, &signature).is_err());
    }
}

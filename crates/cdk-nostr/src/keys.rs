//! Nostr key helpers: generation, parsing and public key derivation

use nostr::prelude::{Keys, PublicKey, SecretKey};

use crate::error::{Error, Result};

/// Generate a fresh random Nostr secret key
pub fn generate_secret_key() -> SecretKey {
    Keys::generate().secret_key().clone()
}

/// Parse a Nostr secret key from hex (64 characters) or bech32 `nsec`
///
/// # Errors
///
/// Returns [`Error::InvalidSecretKey`] if the key cannot be parsed or is not a
/// valid secp256k1 scalar.
pub fn parse_secret_key(key: &str) -> Result<SecretKey> {
    SecretKey::parse(key).map_err(|e| Error::InvalidSecretKey(e.to_string()))
}

/// Parse a x-only Nostr public key from hex (64 characters) or bech32 `npub`
///
/// # Errors
///
/// Returns [`Error::InvalidPublicKey`] if the key cannot be parsed.
pub fn parse_public_key(key: &str) -> Result<PublicKey> {
    PublicKey::parse(key).map_err(|e| Error::InvalidPublicKey(e.to_string()))
}

/// Derive the x-only public key for a secret key
pub fn public_key(secret_key: &SecretKey) -> PublicKey {
    Keys::new(secret_key.clone()).public_key()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_key_of_one_is_generator_point() {
        let secret =
            parse_secret_key("0000000000000000000000000000000000000000000000000000000000000001")
                .expect("valid secret key");
        assert_eq!(
            public_key(&secret).to_hex(),
            "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
        );
    }

    #[test]
    fn parse_secret_key_rejects_invalid() {
        assert!(parse_secret_key("zzzz").is_err());
        assert!(parse_secret_key(&"00".repeat(32)).is_err());
    }

    #[test]
    fn generate_secret_key_roundtrips() {
        let secret = generate_secret_key();
        let parsed = parse_secret_key(&secret.to_secret_hex()).expect("generated key parses");
        assert_eq!(parsed.to_secret_hex(), secret.to_secret_hex());
    }
}

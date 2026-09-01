//! Nostr key helpers: generation, parsing and public key derivation

use bitcoin::bip32::{DerivationPath, Xpriv};
use bitcoin::Network;
use nostr_sdk::nips::nip06::{Error as Nip06Error, FromMnemonic};
use nostr_sdk::{Keys, PublicKey, SecretKey};

use crate::error::{Error, Result};

/// Default NIP-06 identity derivation path.
pub const DEFAULT_NIP06_DERIVATION_PATH: &str = "m/44'/1237'/0'/0/0";

/// Derive the default NIP-06 Nostr secret key from a 64-byte BIP-39 seed.
///
/// This uses [`DEFAULT_NIP06_DERIVATION_PATH`]. The seed remains in Rust and
/// only the derived Nostr key is returned.
///
/// # Errors
///
/// Returns [`Error::KeyDerivation`] if the BIP-32 root or child key cannot be
/// derived.
pub fn derive_nip06_secret_key_from_seed(seed: &[u8; 64]) -> Result<SecretKey> {
    let path = DEFAULT_NIP06_DERIVATION_PATH
        .parse::<DerivationPath>()
        .map_err(|e| Error::KeyDerivation(e.to_string()))?;

    let root = Xpriv::new_master(Network::Bitcoin, seed)
        .map_err(|e| Error::KeyDerivation(e.to_string()))?;
    let child = root
        .derive_priv(nostr_sdk::SECP256K1, &path)
        .map_err(|e| Error::KeyDerivation(e.to_string()))?;

    SecretKey::from_slice(&child.private_key.secret_bytes())
        .map_err(|e| Error::KeyDerivation(e.to_string()))
}

/// Derive the default NIP-06 Nostr keys from a BIP-39 mnemonic and optional
/// passphrase.
///
/// The mnemonic is parsed and expanded to its BIP-39 seed entirely in Rust.
///
/// # Errors
///
/// Returns [`Error::InvalidMnemonic`] for an invalid mnemonic, or
/// [`Error::KeyDerivation`] if NIP-06 derivation fails.
pub fn derive_nip06_keys_from_mnemonic(mnemonic: &str, passphrase: Option<&str>) -> Result<Keys> {
    Keys::from_mnemonic(mnemonic, passphrase).map_err(|e| match e {
        Nip06Error::BIP39(e) => Error::InvalidMnemonic(e.to_string()),
        Nip06Error::BIP32(e) => Error::KeyDerivation(e.to_string()),
    })
}

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
    use bip39::Mnemonic;

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
    fn public_nip06_mnemonic_matches_vector() {
        let keys = derive_nip06_keys_from_mnemonic(
            "leader monkey parrot ring guide accident before fence cannon height naive bean",
            None,
        )
        .expect("public NIP-06 mnemonic derives");

        assert_eq!(
            keys.secret_key().to_secret_hex(),
            "7f7ff03d123792d6ac594bfa67bf6d0c0ab55b6b1fdb6249303fe861f1ccba9a"
        );
    }

    #[test]
    fn mnemonic_and_seed_derivation_match() {
        let mnemonic = Mnemonic::parse_normalized(
            "leader monkey parrot ring guide accident before fence cannon height naive bean",
        )
        .expect("valid mnemonic");
        let seed = mnemonic.to_seed_normalized("cashu");
        let from_mnemonic = derive_nip06_keys_from_mnemonic(
            "leader monkey parrot ring guide accident before fence cannon height naive bean",
            Some("cashu"),
        )
        .expect("mnemonic derives");
        let from_seed = derive_nip06_secret_key_from_seed(&seed).expect("seed derives");

        assert_eq!(from_mnemonic.secret_key(), &from_seed);
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

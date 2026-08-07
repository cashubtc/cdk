//! NUT-342: Efficient wallet recovery with dynamic gap backup.

use std::collections::BTreeMap;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes128Gcm, Nonce};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use thiserror::Error;

use super::nut01::SecretKey;
use crate::util::hex;

/// Metadata carried by blinded messages and their signatures.
pub type Metadata = BTreeMap<String, String>;

/// NUT-342 mint settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Settings {
    /// Whether dynamic-gap metadata is supported.
    pub supported: bool,
}

/// NUT-342 error.
#[derive(Debug, Error)]
pub enum Error {
    /// Metadata is not the required lowercase 40-character hexadecimal value.
    #[error("invalid NUT-342 metadata")]
    InvalidMetadata,
    /// HKDF output expansion failed.
    #[error("could not derive NUT-342 encryption material")]
    KeyDerivation,
    /// AES-GCM encryption or authentication failed.
    #[error("NUT-342 encryption or authentication failed")]
    Encryption,
}

const KEY_INFO: &[u8] = b"cashu:nut-342:d-gap:key:v1";
const NONCE_INFO: &[u8] = b"cashu:nut-342:d-gap:nonce:v1";

fn key_and_nonce(r: &SecretKey) -> Result<([u8; 16], [u8; 12]), Error> {
    let hkdf = Hkdf::<Sha256>::new(Some(&[]), &r.to_secret_bytes());
    let mut key = [0_u8; 16];
    let mut nonce = [0_u8; 12];
    hkdf.expand(KEY_INFO, &mut key)
        .map_err(|_| Error::KeyDerivation)?;
    hkdf.expand(NONCE_INFO, &mut nonce)
        .map_err(|_| Error::KeyDerivation)?;
    Ok((key, nonce))
}

/// Encrypt a dynamic gap with the output's private blinding factor.
pub fn encrypt_d_gap(d_gap: u32, r: &SecretKey) -> Result<String, Error> {
    let (key, nonce) = key_and_nonce(r)?;
    let cipher = Aes128Gcm::new_from_slice(&key).map_err(|_| Error::Encryption)?;
    let encrypted = cipher
        .encrypt(Nonce::from_slice(&nonce), d_gap.to_be_bytes().as_ref())
        .map_err(|_| Error::Encryption)?;
    Ok(hex::encode(encrypted))
}

/// Decrypt a dynamic gap with the output's private blinding factor.
pub fn decrypt_d_gap(value: &str, r: &SecretKey) -> Result<u32, Error> {
    validate_metadata(value)?;
    let encrypted = hex::decode(value).map_err(|_| Error::InvalidMetadata)?;
    let (key, nonce) = key_and_nonce(r)?;
    let cipher = Aes128Gcm::new_from_slice(&key).map_err(|_| Error::Encryption)?;
    let plaintext = cipher
        .decrypt(Nonce::from_slice(&nonce), encrypted.as_ref())
        .map_err(|_| Error::Encryption)?;
    let bytes: [u8; 4] = plaintext.try_into().map_err(|_| Error::Encryption)?;
    Ok(u32::from_be_bytes(bytes))
}

/// Validate the wire encoding of metadata key `342`.
pub fn validate_metadata(value: &str) -> Result<(), Error> {
    if value.len() != 40
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::InvalidMetadata);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn d_gap_roundtrip_and_validation() {
        let r = SecretKey::generate();
        let encrypted = encrypt_d_gap(42, &r).unwrap();
        assert_eq!(encrypted.len(), 40);
        assert_eq!(decrypt_d_gap(&encrypted, &r).unwrap(), 42);
        assert!(validate_metadata(&encrypted.to_uppercase()).is_err());
    }

    #[test]
    fn encryption_is_deterministic_for_an_output() {
        let r = SecretKey::generate();
        assert_eq!(encrypt_d_gap(7, &r).unwrap(), encrypt_d_gap(7, &r).unwrap());
    }

    #[test]
    fn authentication_rejects_tampering_and_wrong_blinding_factor() {
        let r = SecretKey::generate();
        let encrypted = encrypt_d_gap(7, &r).unwrap();
        assert!(decrypt_d_gap(&encrypted, &SecretKey::generate()).is_err());

        let mut tampered = encrypted.into_bytes();
        tampered[0] = if tampered[0] == b'0' { b'1' } else { b'0' };
        let tampered = String::from_utf8(tampered).unwrap();
        assert!(decrypt_d_gap(&tampered, &r).is_err());
    }

    #[test]
    fn validation_rejects_wrong_length_and_non_hex() {
        assert!(validate_metadata("00").is_err());
        assert!(validate_metadata("gggggggggggggggggggggggggggggggggggggggg").is_err());
    }
}

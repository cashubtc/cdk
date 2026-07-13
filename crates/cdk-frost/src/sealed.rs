//! Encryption-only persistence boundary for FROST secret material.

use std::fmt;

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use frost_secp256k1_tr::rand_core::{CryptoRng, RngCore};
use zeroize::Zeroizing;

use crate::Error;

const SEALED_MAGIC: &[u8; 8] = b"CDKFSE01";
const NONCE_LEN: usize = 12;
const MAX_ENCRYPTED_SECRET_BYTES: usize = 256 * 1024;

/// Participant-local encryption key. Debug output is always redacted.
pub struct SecretEncryptionKey(Zeroizing<[u8; 32]>);

impl SecretEncryptionKey {
    /// Import key bytes derived by the participant's sealed-storage provider.
    pub fn new(bytes: [u8; 32]) -> Result<Self, Error> {
        if bytes == [0; 32] {
            return Err(Error::malformed(
                "FROST secret encryption key",
                "key cannot be all zeroes",
            ));
        }
        Ok(Self(Zeroizing::new(bytes)))
    }
}

impl fmt::Debug for SecretEncryptionKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretEncryptionKey(<redacted>)")
    }
}

/// Versioned authenticated ciphertext suitable for participant-local storage.
#[derive(Clone, PartialEq, Eq)]
pub struct EncryptedSecret(Vec<u8>);

impl EncryptedSecret {
    /// Parse a bounded encrypted record without decrypting it.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, Error> {
        if bytes.len() > MAX_ENCRYPTED_SECRET_BYTES {
            return Err(Error::OversizedInput {
                kind: "encrypted FROST secret",
                max: MAX_ENCRYPTED_SECRET_BYTES,
            });
        }
        if bytes.len() < SEALED_MAGIC.len() + NONCE_LEN + 16
            || &bytes[..SEALED_MAGIC.len()] != SEALED_MAGIC
        {
            return Err(Error::malformed(
                "encrypted FROST secret",
                "truncated or unsupported sealed format",
            ));
        }
        Ok(Self(bytes))
    }

    /// Return ciphertext bytes. No plaintext secret is exposed.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.0.clone()
    }
}

impl fmt::Debug for EncryptedSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EncryptedSecret")
            .field("ciphertext_bytes", &self.0.len())
            .finish()
    }
}

pub(crate) fn seal<R>(
    key: &SecretEncryptionKey,
    aad: &[u8],
    plaintext: &[u8],
    rng: &mut R,
) -> Result<EncryptedSecret, Error>
where
    R: CryptoRng + RngCore,
{
    let mut nonce = [0; NONCE_LEN];
    rng.fill_bytes(&mut nonce);
    let cipher = Aes256Gcm::new_from_slice(key.0.as_ref())
        .map_err(|_| Error::Internal("invalid AES-256-GCM key length".to_string()))?;
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| Error::Internal("FROST secret sealing failed".to_string()))?;
    let mut encoded = Vec::with_capacity(SEALED_MAGIC.len() + NONCE_LEN + ciphertext.len());
    encoded.extend_from_slice(SEALED_MAGIC);
    encoded.extend_from_slice(&nonce);
    encoded.extend_from_slice(&ciphertext);
    EncryptedSecret::from_bytes(encoded)
}

pub(crate) fn open(
    key: &SecretEncryptionKey,
    aad: &[u8],
    encrypted: &EncryptedSecret,
) -> Result<Zeroizing<Vec<u8>>, Error> {
    let nonce_start = SEALED_MAGIC.len();
    let ciphertext_start = nonce_start + NONCE_LEN;
    let cipher = Aes256Gcm::new_from_slice(key.0.as_ref())
        .map_err(|_| Error::Internal("invalid AES-256-GCM key length".to_string()))?;
    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(&encrypted.0[nonce_start..ciphertext_start]),
            Payload {
                msg: &encrypted.0[ciphertext_start..],
                aad,
            },
        )
        .map_err(|_| Error::malformed("encrypted FROST secret", "authentication failed"))?;
    Ok(Zeroizing::new(plaintext))
}

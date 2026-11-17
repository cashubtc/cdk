//! Language-neutral bindings for the existing `cashu` cryptographic primitives.
//!
//! Foreign values are converted here; protocol behavior remains in `cashu`.
//! Consumer-specific output assembly and compatibility rules belong in adapters.

use std::fmt;
use std::str::FromStr;

use cashu::nuts::{BlindSignature, BlindSignatureDleq, Id, PublicKey, SecretKey};
use cashu::secret::Secret;

uniffi::setup_scaffolding!();

/// Invalid representations and failed cryptographic operations.
/// Error messages identify fields without disclosing their contents.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum CryptoError {
    /// A foreign value could not be converted to a core type.
    #[error("Invalid {field}")]
    InvalidInput {
        /// Invalid field name, without its value.
        field: String,
    },
    /// The core could not perform the requested operation.
    #[error("Failed to {operation}")]
    OperationFailed {
        /// Operation name, without input values.
        operation: String,
    },
    /// Signature verification requires a DLEQ proof.
    #[error("Missing DLEQ proof")]
    MissingDleq,
    /// The supplied DLEQ proof failed verification.
    #[error("Invalid DLEQ proof")]
    InvalidDleq,
}

fn invalid(field: &str) -> CryptoError {
    CryptoError::InvalidInput {
        field: field.to_owned(),
    }
}

fn failed(operation: &str) -> CryptoError {
    CryptoError::OperationFailed {
        operation: operation.to_owned(),
    }
}

fn public_key(value: &str, field: &str) -> Result<PublicKey, CryptoError> {
    PublicKey::from_str(value).map_err(|_| invalid(field))
}

fn secret_key(value: &str, field: &str) -> Result<SecretKey, CryptoError> {
    SecretKey::from_hex(value).map_err(|_| invalid(field))
}

/// A blinded message and its secret blinding factor.
#[derive(Clone, uniffi::Record)]
pub struct BlindedMessage {
    /// Compressed secp256k1 point, hex encoded.
    pub blinded_message: String,
    /// Secret scalar, hex encoded.
    pub blinding_factor: String,
}

impl fmt::Debug for BlindedMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BlindedMessage")
            .field("material", &"[REDACTED]")
            .finish()
    }
}

/// Deterministically derived secret material. Treat both fields as wallet secrets.
#[derive(Clone, uniffi::Record)]
pub struct DerivedSecret {
    /// UTF-8 bytes of the core's secret representation.
    pub secret: Vec<u8>,
    /// Secret blinding scalar, hex encoded.
    pub blinding_factor: String,
}

impl fmt::Debug for DerivedSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DerivedSecret")
            .field("material", &"[REDACTED]")
            .finish()
    }
}

/// Mint-supplied DLEQ scalars.
#[derive(Debug, Clone, uniffi::Record)]
pub struct DleqProof {
    /// Challenge scalar, hex encoded.
    pub e: String,
    /// Response scalar, hex encoded.
    pub s: String,
}

/// Portable representation of a core blind signature.
#[derive(Debug, Clone, uniffi::Record)]
pub struct Signature {
    /// Signed denomination.
    pub amount: u64,
    /// Full keyset ID.
    pub keyset_id: String,
    /// Compressed blinded signature, hex encoded.
    pub blinded_signature: String,
    /// Optional proof; verification reports an error when absent.
    pub dleq: Option<DleqProof>,
}

/// Generate a secret using the core's OS-backed random generator.
#[uniffi::export]
pub fn generate_secret() -> Vec<u8> {
    Secret::generate().to_bytes()
}

/// Generate a nonzero secret scalar, encoded as hex.
#[uniffi::export]
pub fn generate_secret_key() -> String {
    SecretKey::generate().to_secret_hex()
}

/// Parse a compressed public key and return its canonical hex representation.
#[uniffi::export]
pub fn normalize_public_key(key: String) -> Result<String, CryptoError> {
    public_key(&key, "public key").map(|key| key.to_string())
}

/// Derive the compressed public key for a hex-encoded secret scalar.
#[uniffi::export]
pub fn derive_public_key(key: String) -> Result<String, CryptoError> {
    secret_key(&key, "secret key").map(|key| key.public_key().to_string())
}

/// Blind arbitrary message bytes; generate a blinding factor when none is given.
#[uniffi::export]
pub fn blind_message(
    secret: Vec<u8>,
    blinding_factor: Option<String>,
) -> Result<BlindedMessage, CryptoError> {
    let r = blinding_factor
        .as_deref()
        .map(|r| secret_key(r, "blinding factor"))
        .transpose()?;
    let (message, r) =
        cashu::dhke::blind_message(&secret, r).map_err(|_| failed("blind message"))?;
    Ok(BlindedMessage {
        blinded_message: message.to_string(),
        blinding_factor: r.to_secret_hex(),
    })
}

/// Unblind a signature using the core primitive. This does not verify a DLEQ proof.
#[uniffi::export]
pub fn unblind_message(
    blinded_signature: String,
    blinding_factor: String,
    mint_key: String,
) -> Result<String, CryptoError> {
    let c = public_key(&blinded_signature, "blinded signature")?;
    let r = secret_key(&blinding_factor, "blinding factor")?;
    let key = public_key(&mint_key, "mint public key")?;
    cashu::dhke::unblind_message(&c, &r, &key)
        .map(|c| c.to_string())
        .map_err(|_| failed("unblind message"))
}

/// Verify a blind signature's DLEQ proof. Missing proofs are errors, as in `cashu`.
#[uniffi::export]
pub fn verify_dleq(
    signature: Signature,
    blinded_message: String,
    mint_key: String,
) -> Result<(), CryptoError> {
    let dleq = signature.dleq.ok_or(CryptoError::MissingDleq)?;
    let signature = BlindSignature {
        amount: signature.amount.into(),
        keyset_id: Id::from_str(&signature.keyset_id).map_err(|_| invalid("keyset ID"))?,
        c: public_key(&signature.blinded_signature, "blinded signature")?,
        dleq: Some(BlindSignatureDleq {
            e: secret_key(&dleq.e, "DLEQ challenge")?,
            s: secret_key(&dleq.s, "DLEQ response")?,
        }),
    };
    let key = public_key(&mint_key, "mint public key")?;
    let message = public_key(&blinded_message, "blinded message")?;
    signature
        .verify_dleq(key, message)
        .map_err(|_| CryptoError::InvalidDleq)
}

/// Derive NUT-13 material with the core's 64-byte seed and u32 counter interface.
/// The core enforces the version-specific derivation limits.
#[uniffi::export]
pub fn derive_secret(
    seed: Vec<u8>,
    keyset_id: String,
    counter: u32,
) -> Result<DerivedSecret, CryptoError> {
    let seed: [u8; 64] = seed
        .try_into()
        .map_err(|_| invalid("seed length (64 bytes)"))?;
    let id = Id::from_str(&keyset_id).map_err(|_| invalid("keyset ID"))?;
    let secret = Secret::from_seed(&seed, id, counter).map_err(|_| failed("derive secret"))?;
    let r =
        SecretKey::from_seed(&seed, id, counter).map_err(|_| failed("derive blinding factor"))?;
    Ok(DerivedSecret {
        secret: secret.to_bytes(),
        blinding_factor: r.to_secret_hex(),
    })
}

/// Blind one NUT-28 public key at an explicit canonical slot.
/// Callers choose key ordering and whether to reuse an ephemeral secret.
#[uniffi::export]
pub fn blind_public_key(
    key: String,
    ephemeral_secret: String,
    canonical_slot: u8,
) -> Result<String, CryptoError> {
    let key = public_key(&key, "public key")?;
    let e = secret_key(&ephemeral_secret, "ephemeral secret")?;
    let r = cashu::nuts::nut28::ecdh_kdf(&e, &key, canonical_slot)
        .map_err(|_| failed("derive key blinding factor"))?;
    cashu::nuts::nut28::blind_public_key(&key, &r)
        .map(|key| key.to_string())
        .map_err(|_| failed("blind public key"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "009a1f293253e41e";

    #[test]
    fn round_trip_signature_and_require_valid_dleq() {
        let secret = generate_secret();
        let output = blind_message(secret.clone(), None).unwrap();
        let mint_secret = SecretKey::generate();
        let key = mint_secret.public_key().to_string();
        let b = PublicKey::from_str(&output.blinded_message).unwrap();
        let c = cashu::dhke::sign_message(&mint_secret, &b).unwrap();
        let signed =
            BlindSignature::new(8.into(), c, ID.parse().unwrap(), &b, &mint_secret).unwrap();
        let dleq = signed.dleq.unwrap();
        let mut signature = Signature {
            amount: 8,
            keyset_id: ID.to_owned(),
            blinded_signature: c.to_string(),
            dleq: Some(DleqProof {
                e: dleq.e.to_secret_hex(),
                s: dleq.s.to_secret_hex(),
            }),
        };
        verify_dleq(
            signature.clone(),
            output.blinded_message.clone(),
            key.clone(),
        )
        .unwrap();
        let proof =
            unblind_message(c.to_string(), output.blinding_factor.clone(), key.clone()).unwrap();
        let expected =
            cashu::dhke::sign_message(&mint_secret, &cashu::dhke::hash_to_curve(&secret).unwrap())
                .unwrap();
        assert_eq!(proof, expected.to_string());
        signature.dleq.as_mut().unwrap().s = generate_secret_key();
        assert!(matches!(
            verify_dleq(
                signature.clone(),
                output.blinded_message.clone(),
                key.clone()
            ),
            Err(CryptoError::InvalidDleq)
        ));
        signature.dleq = None;
        assert!(matches!(
            verify_dleq(signature, output.blinded_message.clone(), key),
            Err(CryptoError::MissingDleq)
        ));
        assert!(!format!("{output:?}").contains(&output.blinding_factor));
    }

    #[test]
    fn derivation_preserves_the_core_input_domain() {
        for id in [ID.to_owned(), format!("01{}", "ab".repeat(32))] {
            let seed = [42; 64];
            let material = derive_secret(seed.to_vec(), id.clone(), 12).unwrap();
            let keyset = id.parse().unwrap();
            assert_eq!(
                material.secret,
                Secret::from_seed(&seed, keyset, 12).unwrap().to_bytes()
            );
            assert_eq!(
                material.blinding_factor,
                SecretKey::from_seed(&seed, keyset, 12)
                    .unwrap()
                    .to_secret_hex()
            );
            assert!(!format!("{material:?}").contains(&material.blinding_factor));
        }
        assert!(derive_secret(vec![42; 32], ID.to_owned(), 0).is_err());
        assert!(derive_secret(vec![42; 64], ID.to_owned(), 1 << 31).is_err());
        assert!(derive_secret(vec![42; 64], format!("01{}", "ab".repeat(32)), u32::MAX).is_ok());
    }

    #[test]
    fn key_blinding_uses_the_requested_core_slot() {
        let e = SecretKey::generate();
        let key = SecretKey::generate().public_key();
        for slot in [0, 1, 10] {
            let r = cashu::nuts::nut28::ecdh_kdf(&e, &key, slot).unwrap();
            let expected = cashu::nuts::nut28::blind_public_key(&key, &r).unwrap();
            assert_eq!(
                blind_public_key(key.to_string(), e.to_secret_hex(), slot).unwrap(),
                expected.to_string()
            );
        }
        assert!(blind_public_key(key.to_string(), e.to_secret_hex(), 11).is_err());
        assert!(normalize_public_key(key.to_string()[2..].to_owned()).is_err());
        assert!(blind_message(vec![0, 255], Some("invalid".to_owned())).is_err());
        assert!(blind_message(vec![0, 255], None).is_ok());
    }
}

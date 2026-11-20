//! Language-neutral bindings for the existing `cashu` cryptographic primitives.
//!
//! Foreign values are converted here; protocol behavior remains in `cashu`.
//! Consumer-specific output assembly and compatibility rules belong in adapters.

use std::fmt;

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

fn public_key(value: &[u8], field: &str) -> Result<PublicKey, CryptoError> {
    // The core byte parser also accepts uncompressed points.
    if value.len() != 33 {
        return Err(invalid(field));
    }
    PublicKey::from_slice(value).map_err(|_| invalid(field))
}

fn secret_key(value: &[u8], field: &str) -> Result<SecretKey, CryptoError> {
    SecretKey::from_slice(value).map_err(|_| invalid(field))
}

/// A blinded message and its secret blinding factor.
#[derive(Clone, uniffi::Record)]
pub struct BlindedMessage {
    /// 33-byte compressed secp256k1 point.
    pub blinded_message: Vec<u8>,
    /// 32-byte big-endian secret scalar.
    pub blinding_factor: Vec<u8>,
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
    /// 32-byte big-endian secret blinding scalar.
    pub blinding_factor: Vec<u8>,
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
    /// 32-byte big-endian challenge scalar.
    pub e: Vec<u8>,
    /// 32-byte big-endian response scalar.
    pub s: Vec<u8>,
}

/// Portable representation of a core blind signature.
#[derive(Debug, Clone, uniffi::Record)]
pub struct Signature {
    /// Signed denomination.
    pub amount: u64,
    /// Full keyset ID, including the version byte.
    pub keyset_id: Vec<u8>,
    /// 33-byte compressed blinded signature.
    pub blinded_signature: Vec<u8>,
    /// Optional proof; verification reports an error when absent.
    pub dleq: Option<DleqProof>,
}

/// Generate a secret using the core's OS-backed random generator.
#[uniffi::export]
pub fn generate_secret() -> Vec<u8> {
    Secret::generate().to_bytes()
}

/// Generate a nonzero secret scalar as 32 big-endian bytes.
#[uniffi::export]
pub fn generate_secret_key() -> Vec<u8> {
    SecretKey::generate().as_secret_bytes().to_vec()
}

/// Parse a compressed public key and return its canonical 33-byte representation.
#[uniffi::export]
pub fn normalize_public_key(key: Vec<u8>) -> Result<Vec<u8>, CryptoError> {
    public_key(&key, "public key").map(|key| key.to_bytes().to_vec())
}

/// Derive the compressed public key for a 32-byte big-endian secret scalar.
#[uniffi::export]
pub fn derive_public_key(key: Vec<u8>) -> Result<Vec<u8>, CryptoError> {
    secret_key(&key, "secret key").map(|key| key.public_key().to_bytes().to_vec())
}

/// Blind arbitrary message bytes; generate a blinding factor when none is given.
#[uniffi::export]
pub fn blind_message(
    secret: Vec<u8>,
    blinding_factor: Option<Vec<u8>>,
) -> Result<BlindedMessage, CryptoError> {
    let r = blinding_factor
        .as_deref()
        .map(|r| secret_key(r, "blinding factor"))
        .transpose()?;
    let (message, r) =
        cashu::dhke::blind_message(&secret, r).map_err(|_| failed("blind message"))?;
    Ok(BlindedMessage {
        blinded_message: message.to_bytes().to_vec(),
        blinding_factor: r.as_secret_bytes().to_vec(),
    })
}

/// Unblind a signature using the core primitive. This does not verify a DLEQ proof.
#[uniffi::export]
pub fn unblind_message(
    blinded_signature: Vec<u8>,
    blinding_factor: Vec<u8>,
    mint_key: Vec<u8>,
) -> Result<Vec<u8>, CryptoError> {
    let c = public_key(&blinded_signature, "blinded signature")?;
    let r = secret_key(&blinding_factor, "blinding factor")?;
    let key = public_key(&mint_key, "mint public key")?;
    cashu::dhke::unblind_message(&c, &r, &key)
        .map(|c| c.to_bytes().to_vec())
        .map_err(|_| failed("unblind message"))
}

/// Verify a blind signature's DLEQ proof. Missing proofs are errors, as in `cashu`.
#[uniffi::export]
pub fn verify_dleq(
    signature: Signature,
    blinded_message: Vec<u8>,
    mint_key: Vec<u8>,
) -> Result<(), CryptoError> {
    let dleq = signature.dleq.ok_or(CryptoError::MissingDleq)?;
    let signature = BlindSignature {
        amount: signature.amount.into(),
        keyset_id: Id::from_bytes(&signature.keyset_id).map_err(|_| invalid("keyset ID"))?,
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
    keyset_id: Vec<u8>,
    counter: u32,
) -> Result<DerivedSecret, CryptoError> {
    let seed: [u8; 64] = seed
        .try_into()
        .map_err(|_| invalid("seed length (64 bytes)"))?;
    let id = Id::from_bytes(&keyset_id).map_err(|_| invalid("keyset ID"))?;
    let secret = Secret::from_seed(&seed, id, counter).map_err(|_| failed("derive secret"))?;
    let r =
        SecretKey::from_seed(&seed, id, counter).map_err(|_| failed("derive blinding factor"))?;
    Ok(DerivedSecret {
        secret: secret.to_bytes(),
        blinding_factor: r.as_secret_bytes().to_vec(),
    })
}

/// Blind one NUT-28 public key at an explicit canonical slot.
/// Callers choose key ordering and whether to reuse an ephemeral secret.
#[uniffi::export]
pub fn blind_public_key(
    key: Vec<u8>,
    ephemeral_secret: Vec<u8>,
    canonical_slot: u8,
) -> Result<Vec<u8>, CryptoError> {
    let key = public_key(&key, "public key")?;
    let e = secret_key(&ephemeral_secret, "ephemeral secret")?;
    let r = cashu::nuts::nut28::ecdh_kdf(&e, &key, canonical_slot)
        .map_err(|_| failed("derive key blinding factor"))?;
    cashu::nuts::nut28::blind_public_key(&key, &r)
        .map(|key| key.to_bytes().to_vec())
        .map_err(|_| failed("blind public key"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEYSET_ID: [u8; 8] = [0; 8];

    #[test]
    fn output_records_expose_bytes_and_redact_debug() {
        let derived = derive_secret(vec![42; 64], KEYSET_ID.to_vec(), 0).unwrap();
        assert_eq!(derived.blinding_factor.len(), 32);
        assert_eq!(derived.secret.len(), 64);
        assert!(derived.secret.iter().all(u8::is_ascii_hexdigit));
        assert_eq!(
            format!("{derived:?}"),
            r#"DerivedSecret { material: "[REDACTED]" }"#
        );

        let output = blind_message(derived.secret, Some(derived.blinding_factor.clone())).unwrap();
        assert_eq!(output.blinding_factor, derived.blinding_factor);
        assert_eq!(output.blinded_message.len(), 33);
        assert_eq!(
            format!("{output:?}"),
            r#"BlindedMessage { material: "[REDACTED]" }"#
        );
    }

    #[test]
    fn invalid_foreign_inputs_identify_the_field() {
        let uncompressed = SecretKey::generate().public_key().to_uncompressed_bytes();
        assert!(matches!(
            normalize_public_key(uncompressed.to_vec()),
            Err(CryptoError::InvalidInput { field }) if field == "public key"
        ));
        assert!(matches!(
            derive_secret(vec![42; 32], KEYSET_ID.to_vec(), 0),
            Err(CryptoError::InvalidInput { field }) if field == "seed length (64 bytes)"
        ));
        assert!(matches!(
            blind_message(vec![0xff], Some(vec![1; 31])),
            Err(CryptoError::InvalidInput { field }) if field == "blinding factor"
        ));
    }

    #[test]
    fn dleq_errors_distinguish_absent_and_rejected_proofs() {
        let key = derive_public_key(vec![1; 32]).unwrap();
        let mut signature = Signature {
            amount: 1,
            keyset_id: KEYSET_ID.to_vec(),
            blinded_signature: key.clone(),
            dleq: None,
        };
        assert!(matches!(
            verify_dleq(signature.clone(), key.clone(), key.clone()),
            Err(CryptoError::MissingDleq)
        ));
        signature.dleq = Some(DleqProof {
            e: vec![1; 32],
            s: vec![2; 32],
        });
        assert!(matches!(
            verify_dleq(signature, key.clone(), key),
            Err(CryptoError::InvalidDleq)
        ));
    }
}

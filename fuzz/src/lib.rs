//! Shared helpers for CDK fuzz targets.
//!
//! This library is consumed by the `[[bin]]` fuzz targets in `fuzz_targets/`.
//! It keeps deterministic-but-flexible "arbitrary" constructors for the core
//! Cashu protocol types in one place so that structured fuzzers can share
//! coverage-generating wrappers without forcing production crates to depend
//! on the `arbitrary` crate.

pub mod arbitrary_ext;

use cashu::nuts::nut01::{BlsG1PublicKey, BlsG2PublicKey, BlsSecretKey};
use cashu::nuts::nut10::{Kind, SecretData};
use cashu::nuts::SpendingConditions;
use cashu::{Nut10Secret, PublicKey, SecretKey};

/// Build a NUT-10 secret without the construction checks the public
/// conversion applies.
///
/// Verifier fuzzing needs malformed locks (duplicate keys, impossible
/// thresholds) to reach the verifier at all, and the validating `TryFrom` would
/// reject them before they got there.
pub fn nut10_secret_unchecked(conditions: SpendingConditions) -> Nut10Secret {
    match conditions {
        SpendingConditions::P2PKConditions { data, conditions } => {
            Nut10Secret::new(Kind::P2PK, SecretData::new(data.to_hex(), conditions))
        }
        SpendingConditions::HTLCConditions { data, conditions } => {
            Nut10Secret::new(Kind::HTLC, SecretData::new(data.to_string(), conditions))
        }
    }
}

/// Deterministic `SecretKey` from 32 fuzz bytes.
///
/// Falls back to `[1u8; 32]` when the bytes are out of curve order so we
/// never panic in the corpus.
pub fn secret_key_from(bytes: [u8; 32]) -> SecretKey {
    SecretKey::from_slice(&bytes)
        .unwrap_or_else(|_| SecretKey::from_slice(&[1u8; 32]).expect("0x01..01 is a valid scalar"))
}

/// Derive a `PublicKey` from 32 fuzz bytes via `secret_key_from`.
pub fn pubkey_from(bytes: [u8; 32]) -> PublicKey {
    secret_key_from(bytes).public_key()
}

/// A valid BLS12-381 G1 point (compressed 48 bytes) from fuzz bytes.
///
/// `hash_to_curve` always yields a canonical, subgroup-correct, non-identity
/// point, which random byte strings would essentially never decode to.
pub fn bls_g1_point_from(bytes: &[u8]) -> BlsG1PublicKey {
    BlsG1PublicKey::hash_to_curve(bytes)
}

/// A valid non-zero BLS12-381 scalar from 32 fuzz bytes.
///
/// Falls back to `0x01..01` when the bytes are zero or not canonical.
pub fn bls_secret_key_from(bytes: [u8; 32]) -> BlsSecretKey {
    BlsSecretKey::from_bytes(&bytes).unwrap_or_else(|_| {
        BlsSecretKey::from_bytes(&[1u8; 32]).expect("0x01..01 is a valid BLS scalar")
    })
}

/// A valid BLS12-381 G2 mint key (compressed 96 bytes) from 32 fuzz bytes.
pub fn bls_g2_key_from(bytes: [u8; 32]) -> BlsG2PublicKey {
    bls_secret_key_from(bytes).public_key_g2()
}

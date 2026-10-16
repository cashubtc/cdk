//! Shared helpers for CDK fuzz targets.
//!
//! This library is consumed by the `[[bin]]` fuzz targets in `fuzz_targets/`.
//! It keeps deterministic-but-flexible "arbitrary" constructors for the core
//! Cashu protocol types in one place so that structured fuzzers can share
//! coverage-generating wrappers without forcing production crates to depend
//! on the `arbitrary` crate.

pub mod arbitrary_ext;

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
    SecretKey::from_slice(&bytes).unwrap_or_else(|_| {
        SecretKey::from_slice(&[1u8; 32]).expect("0x01..01 is a valid scalar")
    })
}

/// Derive a `PublicKey` from 32 fuzz bytes via `secret_key_from`.
pub fn pubkey_from(bytes: [u8; 32]) -> PublicKey {
    secret_key_from(bytes).public_key()
}

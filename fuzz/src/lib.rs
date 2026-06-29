//! Shared helpers for CDK fuzz targets.
//!
//! This library is consumed by the `[[bin]]` fuzz targets in `fuzz_targets/`.
//! It keeps deterministic-but-flexible "arbitrary" constructors for the core
//! Cashu protocol types in one place so that structured fuzzers can share
//! coverage-generating wrappers without forcing production crates to depend
//! on the `arbitrary` crate.

pub mod arbitrary_ext;

use cashu::nuts::nut01::BlsG1PublicKey;
use cashu::{PublicKey, SecretKey};

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

/// A *valid* BLS12-381 G1 `PublicKey` (compressed 48 bytes) from fuzz bytes.
///
/// `hash_to_curve` always yields a canonical, subgroup-correct, non-identity
/// point, so this deterministically produces a parseable BLS G1 key. Used to
/// inject BLS points into positions that the protocol expects to be
/// secp256k1 (e.g. P2PK/HTLC pubkeys) — the case random hex strings would
/// effectively never hit.
pub fn bls_g1_pubkey_from(bytes: &[u8]) -> PublicKey {
    BlsG1PublicKey::hash_to_curve(bytes).into()
}

/// A *valid* BLS12-381 G2 `PublicKey` (compressed 96 bytes) from 32 fuzz bytes.
///
/// Falls back to a fixed valid scalar when the bytes are non-canonical so the
/// result is always a parseable G2 key.
pub fn bls_g2_pubkey_from(bytes: [u8; 32]) -> PublicKey {
    SecretKey::bls_from_slice(&bytes)
        .unwrap_or_else(|_| SecretKey::bls_from_slice(&[1u8; 32]).expect("valid bls scalar"))
        .public_key()
}

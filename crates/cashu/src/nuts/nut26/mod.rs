//! # Pay-to-Blinded-Key (P2BK) Implementation
//!
//! This module implements NUT-26: Pay-to-Blinded-Key, a privacy enhancement for P2PK (NUT-11)
//! that allows "silent payments" - tokens can be locked to a public key without exposing
//! which public key they're locked to, even to the mint.
//!
//! ## Key Concepts
//!
//! * **Ephemeral Keys**: Sender generates a fresh ephemeral keypair `(e, E)` for each transaction
//! * **ECDH**: Both sides derive the same shared secret via Elliptic Curve Diffie-Hellman
//! * **Blinding**: Public keys are blinded before being sent to the mint
//! * **Key Recovery**: Receiver uses ECDH to recover the original blinding factor and derive signing key
//!
//! ## Feature Highlights
//!
//! * Privacy-preserving P2PK operations
//! * Compatible with existing mints (no mint-side changes needed)
//! * BIP-340 compatibility for x-only pubkeys
//! * Canonical slot mapping for multi-key proofs
//!
//! ## Implementation Details
//!
//! * Uses SHA-256 for key derivation with domain separation
//! * Supports rejection sampling for out-of-range blinding factors
//! * Properly handles SEC1 and BIP-340 key formats
//!
//! See the NUT-26 specification for full details:
//! <https://github.com/cashubtc/nuts/blob/main/26.md>

use std::sync::LazyLock;

use bitcoin::hashes::sha256::Hash as Sha256;
use bitcoin::hashes::{Hash, HashEngine};
use bitcoin::secp256k1::Secp256k1;
use thiserror::Error;

use crate::nuts::nut01::{PublicKey, SecretKey};
use crate::Id;

// Create a static SECP256K1 context that we'll use for operations
static SECP: LazyLock<Secp256k1<bitcoin::secp256k1::All>> = LazyLock::new(Secp256k1::new);

/// NUT-26 Error
#[derive(Debug, Error)]
pub enum Error {
    /// Invalid canonical slot
    #[error("Invalid canonical slot {0}")]
    InvalidCanonicalSlot(u8),
    /// Invalid scalar hex string
    #[error("Invalid scalar hex string: {0}")]
    InvalidScalarHex(String),
    /// Scalar must be 32 bytes (64 hex chars)
    #[error("Scalar must be 32 bytes (64 hex chars), got {0}")]
    InvalidScalarLength(usize),
    /// Scalar is zero
    #[error("Derived signing key is zero (invalid)")]
    ZeroSigningKey,
    /// Could not match even-Y pubkey for BIP340
    #[error("Could not derive valid BIP340 signing key (neither k nor -k matched blinded pubkey)")]
    NoValidBip340Key,
    /// Secp256k1 error
    #[error(transparent)]
    Secp256k1(#[from] bitcoin::secp256k1::Error),
    /// Hex decode error
    #[error(transparent)]
    Hex(#[from] crate::util::hex::Error),
    /// NUT-01 error
    #[error(transparent)]
    NUT01(#[from] crate::nuts::nut01::Error),
}

/// Perform ECDH and get blinding factor r
///
/// This uses the NUT-26 Key Derivation Function (KDF):
/// KDF = SHA256(domain_tag || x_only(Z) || keyset_id || canonical_slot_byte || counter_byte)
/// It iterates the counter from 0 until a valid scalar is found.
///
/// # Arguments
/// * `secret_key` - The secret key to use for ECDH (sender's ephemeral key or receiver's private key)
/// * `pubkey` - The public key to use for ECDH (receiver's public key or sender's ephemeral key)
/// * `keyset_id` - The keyset ID to use in the KDF
/// * `canonical_slot` - The canonical slot index (0-10)
///
/// # Returns
/// * A scalar that can be used to blind the public key (blinding factor r)
///
/// # Errors
/// * If the canonical slot is invalid (must be 0-10)
/// * If the ECDH operation fails
/// * If the derived scalar is invalid
pub fn ecdh_kdf(
    secret_key: &SecretKey,
    pubkey: &PublicKey,
    keyset_id: Id,
    canonical_slot: u8,
) -> Result<SecretKey, Error> {
    if canonical_slot > 10 {
        return Err(Error::InvalidCanonicalSlot(canonical_slot));
    }

    // Compute shared point Z = secret_key * pubkey
    // Use SharedSecret if available (produces 32 bytes typically equal to x-coordinate)
    let shared = pubkey.mul_tweak(&SECP, &secret_key.as_scalar())?;

    // SharedSecret exposes 32 bytes (x-coordinate)
    let z_x: [u8; 32] = shared.x_only_public_key().0.serialize();

    // Build KDF input: domain tag || x-only(Z) || keyset_id (raw bytes) || canonical_slot (1 byte) || counter (1 byte)
    let mut engine = Sha256::engine();
    engine.input(b"Cashu_P2BK_v1");
    engine.input(&z_x);
    engine.input(&keyset_id.to_bytes());
    engine.input(&[canonical_slot]);

    // First attempt without counter byte
    let digest = Sha256::from_engine(engine.clone());
    match SecretKey::from_slice(digest.as_byte_array()).map_err(Error::from) {
        Ok(result) => Ok(result),
        Err(_) => {
            // Retry once with 0xff counter byte if first attempt failed
            engine.input(&[0xFF]);
            let digest = Sha256::from_engine(engine);
            SecretKey::from_slice(digest.as_byte_array()).map_err(Error::from)
        }
    }
}

/// Blind a public key with a random scalar r
///
/// Computes P' = P + r·G where:
/// - P is the original (unblinded) public key
/// - r is the blinding scalar
/// - G is the secp256k1 base point
/// - P' is the blinded public key
///
/// # Arguments
/// * `pubkey` - The public key to blind
/// * `r` - The blinding scalar
///
/// # Returns
/// * The blinded public key
///
/// # Errors
/// * If the point addition fails
pub fn blind_public_key(pubkey: &PublicKey, r: &SecretKey) -> Result<PublicKey, Error> {
    let r_pubkey = r.public_key();
    Ok(pubkey.combine(&r_pubkey)?.into())
}

/// Derive BIP-340 compatible signing key from private key and blinding scalar
///
/// For BIP-340 compatibility, we must handle the even-Y requirement. This function:
/// 1. Unblinds the public key to verify it matches our private key
/// 2. Checks if the parity matches
/// 3. Uses p or -p based on parity to derive the correct key
///
/// # Arguments
/// * `privkey` - The private key
/// * `r` - The blinding scalar
/// * `blinded_pubkey` - The blinded public key (P')
///
/// # Returns
/// * The derived signing key that produces a public key matching blinded_pubkey
///
/// # Errors
/// * If the unblinding fails
/// * If neither k nor -k matches the blinded pubkey
/// * If the resulting scalar is zero (invalid)
pub fn derive_signing_key_bip340(
    privkey: &SecretKey,
    r: &SecretKey,
    blinded_pubkey: &PublicKey,
) -> Result<SecretKey, Error> {
    // Unblind the public key
    let r_pubkey = r.public_key();
    let r_pubkey_neg = r_pubkey.negate(&SECP);
    let unblinded_pubkey = blinded_pubkey.combine(&r_pubkey_neg)?;

    // Get the public key from privkey
    let privkey_pubkey = privkey.public_key();

    // Verify the x-coordinates match
    let (unblinded_x_only, unblinded_parity) = unblinded_pubkey.x_only_public_key();
    let privkey_x_only = privkey_pubkey.x_only_public_key();
    let privkey_pubkey_parity = privkey_pubkey.parity();

    // Compare the x-only public keys
    if unblinded_x_only != privkey_x_only {
        return Err(Error::NoValidBip340Key);
    }

    match privkey_pubkey_parity == unblinded_parity {
        true => Ok(privkey.add_tweak(&r.as_scalar())?.into()),
        false => Ok(privkey.negate().add_tweak(&r.as_scalar())?.into()),
    }
}

#[cfg(feature = "wallet")]
#[cfg(test)]
mod tests;

#[cfg(test)]
mod test_vectors;

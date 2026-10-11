use bitcoin::hashes::{hmac, sha256, Hash, HashEngine, HmacEngine};
use bitcoin::secp256k1::{PublicKey, SecretKey};

use super::{tagged_hash, Error};
use crate::nuts::nut01::BlsSecretKey;
use crate::nuts::{Id, KeySetVersion};

/// A secp256k1 derivation branch within a proof allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyPurpose {
    /// Internal private key (type `0x00`).
    Internal,
    /// NUMS offset (type `0x02`).
    NumsOffset,
    /// Self-owned leaf key (type `0x03`) at a derivation index.
    Leaf(u32),
}

/// Derive a proof key using NUT-13's framed v3 KDF and rejection sampling.
pub fn derive_key(
    seed: &[u8],
    keyset: Id,
    counter: u64,
    purpose: KeyPurpose,
) -> Result<SecretKey, Error> {
    if keyset.get_version() != KeySetVersion::Version02 {
        return Err(Error::InvalidKeyset);
    }
    let (kind, suffix) = match purpose {
        KeyPurpose::Internal => (0, vec![]),
        KeyPurpose::NumsOffset => (2, vec![]),
        KeyPurpose::Leaf(index) => (3, index.to_be_bytes().to_vec()),
    };
    derive(seed, &keyset.to_bytes(), counter, kind, &suffix, |bytes| {
        SecretKey::from_slice(bytes).ok()
    })
}

/// Derive a BLS blinding scalar (type `0x01`) from a proof allocation.
pub fn derive_blinding_factor(
    seed: &[u8],
    keyset: Id,
    counter: u64,
) -> Result<BlsSecretKey, Error> {
    if keyset.get_version() != KeySetVersion::Version02 {
        return Err(Error::InvalidKeyset);
    }
    derive(seed, &keyset.to_bytes(), counter, 1, &[], |bytes| {
        if bytes.iter().all(|b| *b == 0) {
            None
        } else {
            BlsSecretKey::from_bytes(bytes).ok()
        }
    })
}

/// Derive a quote lock key (type `0x04`), scoped to the mint identity.
/// The caller maintains a separate quote counter for each mint identity.
pub fn derive_quote_key(
    seed: &[u8],
    mint_identity: PublicKey,
    counter: u64,
) -> Result<SecretKey, Error> {
    derive(seed, &mint_identity.serialize(), counter, 4, &[], |bytes| {
        SecretKey::from_slice(bytes).ok()
    })
}

/// Derive a quote id preimage (type `0x05`) on the same quote counter as its lock key.
/// Any 32 bytes is valid, so there is no rejection sampling.
pub fn derive_quote_id_preimage(
    seed: &[u8],
    mint_identity: PublicKey,
    counter: u64,
) -> Result<[u8; 32], Error> {
    derive(seed, &mint_identity.serialize(), counter, 5, &[], |bytes| {
        Some(*bytes)
    })
}

/// NUT-04 derived quote id, `hex(tagged_hash("Cashu_QuoteId", id_preimage))`.
pub fn derived_quote_id(id_preimage: &[u8; 32]) -> String {
    crate::util::hex::encode(tagged_hash("Cashu_QuoteId", id_preimage))
}

fn derive<T, F>(
    seed: &[u8],
    scope: &[u8],
    counter: u64,
    kind: u8,
    suffix: &[u8],
    accept: F,
) -> Result<T, Error>
where
    F: Fn(&[u8; 32]) -> Option<T>,
{
    let mut prefix = b"Cashu_KDF_HMAC_SHA256".to_vec();
    prefix.extend_from_slice(&(scope.len() as u32).to_be_bytes());
    prefix.extend_from_slice(scope);
    prefix.extend_from_slice(&counter.to_be_bytes());
    prefix.push(kind);
    for attempt in 0..=u32::MAX {
        let mut engine = HmacEngine::<sha256::Hash>::new(seed);
        engine.input(&prefix);
        engine.input(&attempt.to_be_bytes());
        engine.input(suffix);
        let mut bytes = hmac::Hmac::<sha256::Hash>::from_engine(engine).to_byte_array();
        let key = accept(&bytes);
        zeroize::Zeroize::zeroize(&mut bytes);
        if let Some(key) = key {
            return Ok(key);
        }
    }
    Err(Error::DerivationExhausted)
}

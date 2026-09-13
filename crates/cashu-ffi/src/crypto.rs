//! Standalone crypto primitives: hashing, blinding, unblinding and DLEQ checks.

use std::collections::BTreeMap;

use bitcoin::hashes::{sha256, Hash};
use cashu::amount::SplitTarget;
use cashu::dhke::{
    blind_message as dhke_blind, hash_to_curve as dhke_hash_to_curve, unblind_message,
};
use cashu::nuts::nut01::{Keys, SecretKey};
use cashu::nuts::nut02::Id;
use cashu::nuts::nut12::{Error as Nut12Error, ProofDleq};
use cashu::nuts::Proof;
use cashu::secret::Secret;
use cashu::Amount;

use crate::error::CashuFfiError;
use crate::types::{parse_keyset_id, parse_pubkey, to_amounts, BlindPair, DleqProof, KeyEntry};

/// SHA-256 of the input.
///
/// Exists as the smallest possible end-to-end check of the binding pipeline.
#[uniffi::export]
pub fn sha256_digest(data: Vec<u8>) -> Vec<u8> {
    sha256::Hash::hash(&data).to_byte_array().to_vec()
}

/// NUT-00 `hash_to_curve`, returning the compressed point.
#[uniffi::export]
pub fn hash_to_curve(message: Vec<u8>) -> Result<Vec<u8>, CashuFfiError> {
    Ok(dhke_hash_to_curve(&message)?.to_bytes().to_vec())
}

/// Blind a secret, optionally with a caller supplied blinding factor.
///
/// Omitting `blinding_factor` draws a fresh one from the system RNG.
#[uniffi::export]
pub fn blind_message(
    secret: Vec<u8>,
    blinding_factor: Option<Vec<u8>>,
) -> Result<BlindPair, CashuFfiError> {
    let factor = match blinding_factor {
        Some(bytes) => Some(parse_secret_key("blindingFactor", &bytes)?),
        None => None,
    };
    let (blinded, r) = dhke_blind(&secret, factor)?;
    Ok(BlindPair {
        blinded_secret: blinded.to_hex(),
        blinding_factor: r.to_secret_hex(),
    })
}

/// Blind a batch of secrets in one crossing.
///
/// Exists because a wallet blinds one secret per output, and the round trip
/// costs more than the blinding for a single one.
#[uniffi::export]
pub fn blind_messages(secrets: Vec<Vec<u8>>) -> Result<Vec<BlindPair>, CashuFfiError> {
    secrets
        .into_iter()
        .map(|secret| blind_message(secret, None))
        .collect()
}

/// NUT-00 unblinding: `C = C_ - r * K`.
#[uniffi::export]
pub fn unblind_signature(
    blinded_signature: String,
    blinding_factor: String,
    mint_pubkey: String,
) -> Result<String, CashuFfiError> {
    let c_blinded = parse_pubkey("blindedSignature", &blinded_signature)?;
    let r = parse_secret_key_hex("blindingFactor", &blinding_factor)?;
    let key = parse_pubkey("mintPubkey", &mint_pubkey)?;
    Ok(unblind_message(&c_blinded, &r, &key)?.to_hex())
}

/// Verify the NUT-12 DLEQ proof carried by an unblinded proof.
///
/// A well-formed proof that does not verify returns `false`; only malformed
/// input raises.
#[uniffi::export]
pub fn verify_proof_dleq(
    secret: String,
    unblinded_signature: String,
    dleq: DleqProof,
    blinding_factor: String,
    mint_pubkey: String,
) -> Result<bool, CashuFfiError> {
    let proof = Proof {
        amount: Amount::ZERO,
        keyset_id: Id::from_bytes(&[0u8; 8])?,
        secret: Secret::new(secret),
        c: parse_pubkey("unblindedSignature", &unblinded_signature)?,
        witness: None,
        dleq: Some(ProofDleq::new(
            parse_secret_key_hex("dleq.e", &dleq.e)?,
            parse_secret_key_hex("dleq.s", &dleq.s)?,
            parse_secret_key_hex("blindingFactor", &blinding_factor)?,
        )),
        p2pk_e: None,
    };

    match proof.verify_dleq(parse_pubkey("mintPubkey", &mint_pubkey)?) {
        Ok(()) => Ok(true),
        Err(Nut12Error::InvalidDleqProof) => Ok(false),
        Err(err) => Err(err.into()),
    }
}

/// Compute the NUT-02 v1 keyset id for a set of mint keys.
#[uniffi::export]
pub fn keyset_id_v1(keys: Vec<KeyEntry>) -> Result<String, CashuFfiError> {
    let mut map = BTreeMap::new();
    for entry in &keys {
        map.insert(
            Amount::from(entry.amount),
            parse_pubkey("keys", &entry.pubkey)?,
        );
    }
    Ok(Id::v1_from_keys(&Keys::new(map)).to_string())
}

/// Split an amount over the denominations a keyset can sign.
///
/// `custom_split` pins specific denominations; any remainder is split greedily.
#[uniffi::export]
pub fn split_amount(
    amount: u64,
    denominations: Vec<u64>,
    custom_split: Option<Vec<u64>>,
) -> Result<Vec<u64>, CashuFfiError> {
    let target = match custom_split {
        Some(values) => SplitTarget::Values(to_amounts(&values)),
        None => SplitTarget::None,
    };
    let fee_and_amounts = (0u64, denominations).into();
    Ok(Amount::from(amount)
        .split_targeted(&target, &fee_and_amounts)?
        .into_iter()
        .map(u64::from)
        .collect())
}

/// Parse a 32 byte secret key, naming the argument it came from.
pub(crate) fn parse_secret_key(field: &str, bytes: &[u8]) -> Result<SecretKey, CashuFfiError> {
    SecretKey::from_slice(bytes).map_err(|err| CashuFfiError::InvalidSecretKey {
        field: field.to_string(),
        reason: err.to_string(),
    })
}

/// Parse a hex encoded secret key, naming the argument it came from.
pub(crate) fn parse_secret_key_hex(field: &str, hex: &str) -> Result<SecretKey, CashuFfiError> {
    SecretKey::from_hex(hex).map_err(|err| CashuFfiError::InvalidSecretKey {
        field: field.to_string(),
        reason: err.to_string(),
    })
}

/// Validate a keyset id argument without building anything from it.
pub(crate) fn check_keyset_id(id: &str) -> Result<Id, CashuFfiError> {
    parse_keyset_id(id)
}

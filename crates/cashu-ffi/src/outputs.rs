//! Blinded output construction, the batch operations React Native calls into.
//!
//! The batch functions take the exact denominations to produce rather than an
//! amount to split. Splitting is cheap and every wallet already has its own
//! policy for it; the expensive part is the blinding and the NUT-13 derivation,
//! and taking the amounts verbatim keeps the caller's ordering, which NUT-13
//! counters depend on.

use cashu::dhke::blind_message;
use cashu::nuts::nut00::{PreMint, PreMintSecrets};
use cashu::nuts::nut01::SecretKey;
use cashu::nuts::nut02::Id;
use cashu::nuts::nut10::{Conditions, SpendingConditions};
use cashu::nuts::nut11::SigFlag as CashuSigFlag;
use cashu::secret::Secret;

use crate::crypto::check_keyset_id;
use crate::error::CashuFfiError;
use crate::types::{parse_pubkey, parse_pubkeys, to_amounts, BlindedOutput, P2pkOptions, SigFlag};

/// Largest restore batch the crate will derive in one call.
///
/// Mirrors cashu-ts `ABSOLUTE_MAX_ARRAY_LENGTH`: a restore batch costs one BIP32
/// derivation and one blinded message per counter, and these calls run on the
/// caller's thread.
pub const MAX_RESTORE_COUNTERS: u32 = 10_000;

/// One random secret per requested denomination.
#[uniffi::export]
pub fn create_random_outputs(
    amounts: Vec<u64>,
    keyset_id: String,
) -> Result<Vec<BlindedOutput>, CashuFfiError> {
    let id = check_keyset_id(&keyset_id)?;
    let secrets: Vec<Secret> = (0..amounts.len()).map(|_| Secret::generate()).collect();
    let pre_mints = PreMintSecrets::from_secrets(id, to_amounts(&amounts), secrets)?;
    Ok(convert(&keyset_id, pre_mints))
}

/// A single random output of exactly `amount`.
#[uniffi::export]
pub fn create_single_random_output(
    amount: u64,
    keyset_id: String,
) -> Result<BlindedOutput, CashuFfiError> {
    let id = check_keyset_id(&keyset_id)?;
    build_output(&keyset_id, id, amount, Secret::generate(), None, None)
}

/// One P2PK locked secret per requested denomination.
#[uniffi::export]
pub fn create_p2pk_outputs(
    p2pk: P2pkOptions,
    amounts: Vec<u64>,
    keyset_id: String,
) -> Result<Vec<BlindedOutput>, CashuFfiError> {
    let id = check_keyset_id(&keyset_id)?;
    let conditions = spending_conditions(&p2pk)?;
    let mut secrets = Vec::with_capacity(amounts.len());
    for _ in &amounts {
        secrets.push(p2pk_secret(&conditions)?);
    }
    let pre_mints = PreMintSecrets::from_secrets(id, to_amounts(&amounts), secrets)?;
    Ok(convert(&keyset_id, pre_mints))
}

/// A single P2PK locked output of exactly `amount`.
#[uniffi::export]
pub fn create_single_p2pk_output(
    p2pk: P2pkOptions,
    amount: u64,
    keyset_id: String,
) -> Result<BlindedOutput, CashuFfiError> {
    let id = check_keyset_id(&keyset_id)?;
    let secret = p2pk_secret(&spending_conditions(&p2pk)?)?;
    build_output(&keyset_id, id, amount, secret, None, None)
}

/// NUT-13 deterministic secrets, one per denomination, walking `counter` upward.
#[uniffi::export]
pub fn create_deterministic_outputs(
    amounts: Vec<u64>,
    seed: Vec<u8>,
    counter: u32,
    keyset_id: String,
) -> Result<Vec<BlindedOutput>, CashuFfiError> {
    let id = check_keyset_id(&keyset_id)?;
    let seed = parse_seed(&seed)?;
    derive_range(&keyset_id, id, &seed, counter, &amounts)
}

/// A single NUT-13 deterministic output of exactly `amount`.
#[uniffi::export]
pub fn create_single_deterministic_output(
    amount: u64,
    seed: Vec<u8>,
    counter: u32,
    keyset_id: String,
) -> Result<BlindedOutput, CashuFfiError> {
    let id = check_keyset_id(&keyset_id)?;
    let seed = parse_seed(&seed)?;
    derive_output(&keyset_id, id, amount, &seed, counter)
}

/// NUT-09 restore batch: `count` zero-amount outputs from `start_counter` up.
#[uniffi::export]
pub fn create_restore_outputs(
    seed: Vec<u8>,
    keyset_id: String,
    start_counter: u32,
    count: u32,
) -> Result<Vec<BlindedOutput>, CashuFfiError> {
    let id = check_keyset_id(&keyset_id)?;
    let seed = parse_seed(&seed)?;
    let amounts = restore_amounts(count)?;
    derive_range(&keyset_id, id, &seed, start_counter, &amounts)
}

/// Blank denominations for a restore batch, refusing a batch that is too large.
pub(crate) fn restore_amounts(count: u32) -> Result<Vec<u64>, CashuFfiError> {
    if count > MAX_RESTORE_COUNTERS {
        return Err(CashuFfiError::InvalidRestoreRange {
            count,
            max: MAX_RESTORE_COUNTERS,
        });
    }
    Ok(vec![0u64; count as usize])
}

/// Derive one output per amount, starting at `counter`.
pub(crate) fn derive_range(
    keyset_id: &str,
    id: Id,
    seed: &[u8; 64],
    counter: u32,
    amounts: &[u64],
) -> Result<Vec<BlindedOutput>, CashuFfiError> {
    let mut outputs = Vec::with_capacity(amounts.len());
    for (offset, amount) in amounts.iter().enumerate() {
        let counter = counter
            .checked_add(offset as u32)
            .ok_or(CashuFfiError::Derivation {
                counter,
                reason: "counter overflowed u32".to_string(),
            })?;
        outputs.push(derive_output(keyset_id, id, *amount, seed, counter)?);
    }
    Ok(outputs)
}

/// Derive one deterministic output, shared by the free functions and the factory.
pub(crate) fn derive_output(
    keyset_id: &str,
    id: Id,
    amount: u64,
    seed: &[u8; 64],
    counter: u32,
) -> Result<BlindedOutput, CashuFfiError> {
    let to_error = |err: cashu::nuts::nut13::Error| CashuFfiError::Derivation {
        counter,
        reason: err.to_string(),
    };
    let secret = Secret::from_seed(seed, id, counter).map_err(to_error)?;
    let blinding_factor = SecretKey::from_seed(seed, id, counter).map_err(to_error)?;
    build_output(
        keyset_id,
        id,
        amount,
        secret,
        Some(blinding_factor),
        Some(counter),
    )
}

/// NUT-13 needs the full 64 byte BIP39 seed, not the 32 byte entropy.
pub(crate) fn parse_seed(seed: &[u8]) -> Result<[u8; 64], CashuFfiError> {
    seed.try_into()
        .map_err(|_| CashuFfiError::InvalidSeedLength {
            length: seed.len() as u64,
        })
}

fn build_output(
    keyset_id: &str,
    _id: Id,
    amount: u64,
    secret: Secret,
    blinding_factor: Option<SecretKey>,
    derivation_index: Option<u32>,
) -> Result<BlindedOutput, CashuFfiError> {
    let (blinded, r) = blind_message(&secret.to_bytes(), blinding_factor)?;
    Ok(BlindedOutput {
        amount,
        keyset_id: keyset_id.to_string(),
        blinded_secret: blinded.to_hex(),
        blinding_factor: r.to_secret_hex(),
        secret: secret.to_string(),
        derivation_index,
    })
}

fn convert(keyset_id: &str, secrets: PreMintSecrets) -> Vec<BlindedOutput> {
    secrets
        .secrets
        .into_iter()
        .map(
            |PreMint {
                 blinded_message,
                 secret,
                 r,
                 amount,
                 derivation_index,
             }| BlindedOutput {
                amount: amount.into(),
                keyset_id: keyset_id.to_string(),
                blinded_secret: blinded_message.blinded_secret.to_hex(),
                blinding_factor: r.to_secret_hex(),
                secret: secret.to_string(),
                derivation_index,
            },
        )
        .collect()
}

fn p2pk_secret(conditions: &SpendingConditions) -> Result<Secret, CashuFfiError> {
    let nut10: cashu::nuts::nut10::Secret = conditions.clone().try_into()?;
    Ok(nut10.try_into()?)
}

/// Translate the flat FFI options into validated NUT-10 spending conditions.
///
/// `Conditions::new` applies the cashu crate's authoring policy, so a locktime
/// in the past or refund keys without a locktime are rejected here rather than
/// producing an output nobody can spend.
fn spending_conditions(p2pk: &P2pkOptions) -> Result<SpendingConditions, CashuFfiError> {
    let data = parse_pubkey("pubkey", &p2pk.pubkey)?;

    let additional = match &p2pk.additional_pubkeys {
        Some(keys) if !keys.is_empty() => Some(parse_pubkeys("additionalPubkeys", keys)?),
        _ => None,
    };
    let refund = match &p2pk.refund_pubkeys {
        Some(keys) if !keys.is_empty() => Some(parse_pubkeys("refundPubkeys", keys)?),
        _ => None,
    };
    let sig_flag = CashuSigFlag::from(p2pk.sig_flag);

    let bare = additional.is_none()
        && refund.is_none()
        && p2pk.num_sigs.is_none()
        && p2pk.locktime.is_none()
        && p2pk.num_sigs_refund.is_none()
        && matches!(p2pk.sig_flag, SigFlag::SigInputs);

    let conditions = if bare {
        None
    } else {
        Some(Conditions::new(
            p2pk.locktime,
            additional,
            refund,
            p2pk.num_sigs,
            Some(sig_flag),
            p2pk.num_sigs_refund,
        )?)
    };

    Ok(SpendingConditions::P2PKConditions { data, conditions })
}

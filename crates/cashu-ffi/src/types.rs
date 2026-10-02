//! Records and enums shared by the exported functions.

use cashu::nuts::nut01::PublicKey;
use cashu::nuts::nut02::Id;
use cashu::nuts::nut11::SigFlag as CashuSigFlag;
use cashu::Amount;

use crate::error::CashuFfiError;

/// One `amount -> mint public key` pair of a keyset.
///
/// A list is used rather than a map because JavaScript object keys are strings,
/// which would silently narrow the u64 amount.
#[derive(Debug, Clone, uniffi::Record)]
pub struct KeyEntry {
    /// Denomination this key signs.
    pub amount: u64,
    /// Compressed secp256k1 public key, hex encoded.
    pub pubkey: String,
}

/// A blinded output plus the secrets needed to later unblind it.
#[derive(Debug, Clone, uniffi::Record)]
pub struct BlindedOutput {
    /// Denomination requested from the mint.
    pub amount: u64,
    /// Keyset the output is addressed to.
    pub keyset_id: String,
    /// `B_`, the blinded secret, hex encoded.
    pub blinded_secret: String,
    /// `r`, the blinding factor, hex encoded.
    pub blinding_factor: String,
    /// The unblinded secret, as the mint will see it after the proof is spent.
    pub secret: String,
    /// NUT-13 counter this output was derived from, when deterministic.
    pub derivation_index: Option<u32>,
}

/// Result of blinding a single secret.
#[derive(Debug, Clone, uniffi::Record)]
pub struct BlindPair {
    /// `B_`, hex encoded.
    pub blinded_secret: String,
    /// `r`, hex encoded.
    pub blinding_factor: String,
}

/// NUT-11 signature flag.
#[derive(Debug, Clone, Copy, uniffi::Enum)]
pub enum SigFlag {
    /// Signatures are required over the inputs only.
    SigInputs,
    /// Signatures are required over inputs and outputs.
    SigAll,
}

impl From<SigFlag> for CashuSigFlag {
    fn from(flag: SigFlag) -> Self {
        match flag {
            SigFlag::SigInputs => Self::SigInputs,
            SigFlag::SigAll => Self::SigAll,
        }
    }
}

/// NUT-11 pay-to-public-key locking options.
#[derive(Debug, Clone, uniffi::Record)]
pub struct P2pkOptions {
    /// Primary locking public key, hex encoded.
    pub pubkey: String,
    /// Further keys that may sign, hex encoded.
    pub additional_pubkeys: Option<Vec<String>>,
    /// Signatures required to spend.
    pub num_sigs: Option<u64>,
    /// Unix timestamp after which the refund keys apply.
    pub locktime: Option<u64>,
    /// Keys that may spend after `locktime`, hex encoded.
    pub refund_pubkeys: Option<Vec<String>>,
    /// Signatures required on the refund branch.
    pub num_sigs_refund: Option<u64>,
    /// Signature flag written into the secret.
    pub sig_flag: SigFlag,
}

/// A NUT-12 DLEQ proof as it appears on a blind signature.
#[derive(Debug, Clone, uniffi::Record)]
pub struct DleqProof {
    /// Challenge `e`, hex encoded.
    pub e: String,
    /// Response `s`, hex encoded.
    pub s: String,
}

/// Parse a keyset id, keeping the rejected value in the error.
pub fn parse_keyset_id(id: &str) -> Result<Id, CashuFfiError> {
    id.parse::<Id>()
        .map_err(|err| CashuFfiError::InvalidKeysetId {
            id: id.to_string(),
            reason: err.to_string(),
        })
}

/// Parse a hex public key, naming the argument it came from.
pub fn parse_pubkey(field: &str, hex: &str) -> Result<PublicKey, CashuFfiError> {
    PublicKey::from_hex(hex).map_err(|err| CashuFfiError::InvalidPublicKey {
        field: field.to_string(),
        reason: err.to_string(),
    })
}

/// Parse a list of hex public keys, naming the argument they came from.
pub fn parse_pubkeys(field: &str, keys: &[String]) -> Result<Vec<PublicKey>, CashuFfiError> {
    keys.iter().map(|key| parse_pubkey(field, key)).collect()
}

/// Convert amounts supplied by the caller into the crate's `Amount` type.
pub fn to_amounts(values: &[u64]) -> Vec<Amount> {
    values.iter().copied().map(Amount::from).collect()
}

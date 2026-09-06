//! Wallet storage and restore types used at the UniFFI boundary.

use cdk_common::bitcoin;

use crate::error::FfiError;
use crate::{Amount, PublicKey};

/// Stored P2PK key metadata exposed by FFI database implementations.
#[derive(Debug, Clone, uniffi::Record)]
pub struct P2PKSigningKey {
    /// Public key.
    pub pubkey: PublicKey,
    /// Derivation path as a string.
    pub derivation_path: String,
    /// Derivation index.
    pub derivation_index: u32,
    /// Creation time as a Unix timestamp.
    pub created_time: u64,
}

impl TryFrom<P2PKSigningKey> for cdk_common::wallet::P2PKSigningKey {
    type Error = FfiError;

    fn try_from(key: P2PKSigningKey) -> Result<Self, Self::Error> {
        Ok(Self {
            pubkey: key.pubkey.try_into()?,
            derivation_path: key.derivation_path.parse().map_err(
                |error: bitcoin::bip32::Error| FfiError::Internal {
                    error_message: error.to_string(),
                },
            )?,
            derivation_index: key.derivation_index,
            created_time: key.created_time,
        })
    }
}

impl From<cdk_common::wallet::P2PKSigningKey> for P2PKSigningKey {
    fn from(key: cdk_common::wallet::P2PKSigningKey) -> Self {
        Self {
            pubkey: key.pubkey.into(),
            derivation_path: key.derivation_path.to_string(),
            derivation_index: key.derivation_index,
            created_time: key.created_time,
        }
    }
}

/// Amounts recovered by a deterministic seed scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct Restored {
    /// Recovered spendable value.
    pub unspent: Amount,
    /// Recovered value still pending at the mint.
    pub pending: Amount,
}

impl From<cdk_common::wallet::Restored> for Restored {
    fn from(value: cdk_common::wallet::Restored) -> Self {
        Self {
            unspent: value.unspent.into(),
            pending: value.pending.into(),
        }
    }
}

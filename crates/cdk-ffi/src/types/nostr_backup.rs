//! FFI types for Nostr mint backup (NUT-27)

use cdk::wallet::advanced::{
    MintBackupReceipt as CdkMintBackupReceipt, MintBackupRequest as CdkMintBackupRequest,
    MintRestorePolicy as CdkMintRestorePolicy, MintRestoreReceipt as CdkMintRestoreReceipt,
    MintRestoreRequest as CdkMintRestoreRequest,
};

use super::MintUrl;

/// Request to publish an encrypted NUT-27 mint backup.
#[derive(Debug, Clone, uniffi::Record)]
pub struct MintBackupRequest {
    /// Relay URLs that receive the backup event.
    pub relays: Vec<String>,
    /// Client name to include in the event tags.
    #[uniffi(default = None)]
    pub client: Option<String>,
}

impl From<MintBackupRequest> for CdkMintBackupRequest {
    fn from(request: MintBackupRequest) -> Self {
        let mut converted = Self::new(request.relays);
        if let Some(client) = request.client {
            converted = converted.with_client(client);
        }
        converted
    }
}

/// Whether restoring a mint backup changes manager configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MintRestorePolicy {
    /// Decrypt and return the backup without registering its mints.
    Preview,
    /// Register every newly discovered mint.
    Register,
}

impl From<MintRestorePolicy> for CdkMintRestorePolicy {
    fn from(value: MintRestorePolicy) -> Self {
        match value {
            MintRestorePolicy::Preview => Self::Preview,
            MintRestorePolicy::Register => Self::Register,
        }
    }
}

/// Request to fetch and decrypt a NUT-27 mint backup.
#[derive(Debug, Clone, uniffi::Record)]
pub struct MintRestoreRequest {
    /// Relay URLs queried for the backup event.
    pub relays: Vec<String>,
    /// Whether discovered mints should be registered.
    pub policy: MintRestorePolicy,
    /// Timeout in seconds for waiting for relay responses.
    #[uniffi(default = 10)]
    pub timeout_seconds: u64,
}

impl From<MintRestoreRequest> for CdkMintRestoreRequest {
    fn from(request: MintRestoreRequest) -> Self {
        Self::new(request.relays)
            .with_policy(request.policy.into())
            .with_timeout(std::time::Duration::from_secs(request.timeout_seconds))
    }
}

/// Receipt for a published mint backup.
#[derive(Debug, Clone, uniffi::Record)]
pub struct MintBackupReceipt {
    /// The event ID of the published backup (hex encoded)
    pub event_id: String,
    /// The public key used for the backup (hex encoded)
    pub public_key: String,
    /// Number of mints backed up
    pub mint_count: u64,
}

impl From<CdkMintBackupReceipt> for MintBackupReceipt {
    fn from(result: CdkMintBackupReceipt) -> Self {
        Self {
            event_id: result.event_id.to_hex(),
            public_key: result.public_key.to_hex(),
            mint_count: result.mint_count as u64,
        }
    }
}

/// Receipt for a fetched and decrypted mint backup.
#[derive(Debug, Clone, uniffi::Record)]
pub struct MintRestoreReceipt {
    /// The restored mint backup data
    pub backup: MintBackup,
    /// Number of mints found in the backup
    pub mint_count: u64,
    /// Number of mints that were newly added (not already in wallet)
    pub mints_added: u64,
}

impl From<CdkMintRestoreReceipt> for MintRestoreReceipt {
    fn from(result: CdkMintRestoreReceipt) -> Self {
        Self {
            backup: result.backup.into(),
            mint_count: result.mint_count as u64,
            mints_added: result.mints_added as u64,
        }
    }
}

/// Mint backup data containing the list of mints and timestamp
#[derive(Debug, Clone, uniffi::Record)]
pub struct MintBackup {
    /// List of mint URLs in the backup
    pub mints: Vec<MintUrl>,
    /// Unix timestamp of when the backup was created
    pub timestamp: u64,
}

impl From<cdk::nuts::nut27::MintBackup> for MintBackup {
    fn from(backup: cdk::nuts::nut27::MintBackup) -> Self {
        Self {
            mints: backup.mints.into_iter().map(|m| m.into()).collect(),
            timestamp: backup.timestamp,
        }
    }
}

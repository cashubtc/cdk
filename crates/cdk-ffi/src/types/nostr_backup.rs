//! FFI types for Nostr mint backup (NUT-27)

use cdk::wallet::{
    BackupOptions as CdkBackupOptions, BackupResult as CdkBackupResult,
    RestoreOptions as CdkRestoreOptions, RestoreResult as CdkRestoreResult,
};

use super::MintUrl;

/// Options for backup operations
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[derive(Debug, Clone, Default)]
pub struct BackupOptions {
    /// Client name to include in the event tags
    pub client: Option<String>,
}

impl From<BackupOptions> for CdkBackupOptions {
    fn from(options: BackupOptions) -> Self {
        let mut opts = CdkBackupOptions::new();
        if let Some(client) = options.client {
            opts = opts.client(client);
        }
        opts
    }
}

/// Options for restore operations
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct RestoreOptions {
    /// Timeout in seconds for waiting for relay responses
    pub timeout_secs: u64,
}

impl Default for RestoreOptions {
    fn default() -> Self {
        Self { timeout_secs: 10 }
    }
}

impl From<RestoreOptions> for CdkRestoreOptions {
    fn from(options: RestoreOptions) -> Self {
        CdkRestoreOptions::new().timeout(std::time::Duration::from_secs(options.timeout_secs))
    }
}

/// Result of a backup operation
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct BackupResult {
    /// The event ID of the published backup (hex encoded)
    pub event_id: String,
    /// The public key used for the backup (hex encoded)
    pub public_key: String,
    /// Number of mints backed up
    pub mint_count: u64,
}

impl From<CdkBackupResult> for BackupResult {
    fn from(result: CdkBackupResult) -> Self {
        Self {
            event_id: result.event_id.to_hex(),
            public_key: result.public_key.to_hex(),
            mint_count: result.mint_count as u64,
        }
    }
}

/// Result of a restore operation
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct RestoreResult {
    /// The restored mint backup data
    pub backup: MintBackup,
    /// Number of mints found in the backup
    pub mint_count: u64,
    /// Number of mints that were newly added (not already in wallet)
    pub mints_added: u64,
}

impl From<CdkRestoreResult> for RestoreResult {
    fn from(result: CdkRestoreResult) -> Self {
        Self {
            backup: result.backup.into(),
            mint_count: result.mint_count as u64,
            mints_added: result.mints_added as u64,
        }
    }
}

/// Mint backup data containing the list of mints and timestamp
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[derive(Debug, Clone)]
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

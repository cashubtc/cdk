//! CDK Database

#[cfg(feature = "mint")]
mod mint;
#[cfg(feature = "wallet")]
mod wallet;

use std::path::{Path, PathBuf};

use cashu::util::unix_time;
#[cfg(feature = "mint")]
pub use mint::Database as MintDatabase;
#[cfg(feature = "wallet")]
pub use wallet::Database as WalletDatabase;

/// CDK_database error
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Database Error
    #[error(transparent)]
    Database(Box<dyn std::error::Error + Send + Sync>),
    /// DHKE error
    #[error(transparent)]
    DHKE(#[from] crate::dhke::Error),
    /// NUT00 Error
    #[error(transparent)]
    NUT00(#[from] crate::nuts::nut00::Error),
    /// NUT02 Error
    #[error(transparent)]
    NUT02(#[from] crate::nuts::nut02::Error),
    /// Serde Error
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
    /// Unknown Quote
    #[error("Unknown Quote")]
    UnknownQuote,
}

/// Creates the backups folder, if missing. Enforces the configured backups limit by removing
/// older backups, if necessary.
///
/// # Arguments
///
/// * `work_dir`: the working directory in which the backups folder will be created
/// * `extension`: the DB backup file extension, which usually indicates the type of DB used
/// * `backups_to_keep`: configured number of backups to keep
///
/// # Returns
///
/// Full path of the new backup, if one is to be created
pub fn prepare_backup(
    work_dir: &Path,
    extension: &str,
    backups_to_keep: u8,
) -> Result<Option<PathBuf>, Error> {
    let prefix = "backup_";

    let backups_dir_path = work_dir.join("backups");
    if !backups_dir_path.exists() {
        std::fs::create_dir_all(&backups_dir_path)
            .map_err(|e| Error::Database(format!("Failed to create backups folder: {e}").into()))?;
    }

    let mut existing_backups: Vec<PathBuf> = std::fs::read_dir(&backups_dir_path)
        .map_err(|e| Error::Database(format!("Failed to list existing backups: {e}").into()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            if let Some(file_name) = path.file_name() {
                if let Some(file_name_str) = file_name.to_str() {
                    return file_name_str.starts_with(prefix)
                        && file_name_str.ends_with(&format!(".{}", extension));
                }
            }
            false
        })
        .collect();

    // Sort backup files by name (which includes timestamp) in descending order
    existing_backups.sort();
    existing_backups.reverse();
    tracing::info!("Found backups: {existing_backups:#?}");

    // Remove excess backups
    tracing::info!("Keeping {backups_to_keep} backups");
    let backup_files_to_delete: Vec<_> = match backups_to_keep as usize {
        0 | 1 => existing_backups.iter().collect(),
        n => existing_backups.iter().skip(n - 1).collect(),
    };
    for backup in backup_files_to_delete {
        tracing::info!("Removing old backup: {:?}", backup);
        std::fs::remove_file(backup)
            .map_err(|e| Error::Database(format!("Failed to remove old backup: {e}").into()))?
    }

    match backups_to_keep {
        0 => Ok(None),
        _ => {
            let new_backup_filename = format!("{}{}.{}", prefix, unix_time(), extension);
            let new_backup_path = backups_dir_path.join(new_backup_filename);
            tracing::info!("New backup file path: {new_backup_path:?}");
            Ok(Some(new_backup_path))
        }
    }
}

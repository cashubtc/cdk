//! SQLite Storage backend for CDK

#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

pub mod error;
mod migrations;

#[cfg(feature = "mint")]
pub mod mint;
#[cfg(feature = "wallet")]
pub mod wallet;

use std::path::{Path, PathBuf};

use cdk_common::database;
#[cfg(feature = "mint")]
pub use mint::MintRedbDatabase;
#[cfg(feature = "wallet")]
pub use wallet::WalletRedbDatabase;

use crate::error::Error;

fn backup(work_dir: &Path, db_file_path: &PathBuf, backups_to_keep: u8) -> Result<(), Error> {
    if let Some(new_backup_file) =
        database::prepare_backup(work_dir, "redb", backups_to_keep).map_err(Error::DbBackup)?
    {
        create_backup(db_file_path, new_backup_file).map_err(Error::DbBackup)?;
    }

    Ok(())
}

fn create_backup(db_file_path: &PathBuf, backup_file_path: PathBuf) -> Result<(), database::Error> {
    // The closest thing to a backup/restore in redb seems to be
    // https://github.com/cberner/redb/issues/100#issuecomment-1371786445
    // However, that is very error prone, as the table list has to be maintained manually.
    //
    // The savepoint feature is also not usable for our use-case, as savepoints are for rolling
    // back transactions and do not allow restoring an older DB schema.
    //
    // Therefore, backups are done by copying the underlying DB file before the DB is opened.

    std::fs::copy(db_file_path, backup_file_path)
        .map_err(|e| database::Error::Database(Box::new(e)))?;

    Ok(())
}

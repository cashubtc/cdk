//! SQLite storage backend for cdk

#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

mod error;
#[cfg(feature = "mint")]
pub mod mint;
#[cfg(feature = "wallet")]
pub mod wallet;

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use cdk_common::database;
#[cfg(feature = "mint")]
pub use mint::MintSqliteDatabase;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Pool, Sqlite};
#[cfg(feature = "wallet")]
pub use wallet::WalletSqliteDatabase;

use crate::error::Error;

async fn connect_to_db(db_file_path: &Path) -> Result<Pool<Sqlite>, Error> {
    let db_file_path_str = db_file_path.to_str().ok_or(Error::InvalidDbPath)?;
    let db_options = SqliteConnectOptions::from_str(db_file_path_str)?
        .busy_timeout(Duration::from_secs(5))
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .read_only(false)
        .create_if_missing(true)
        .auto_vacuum(sqlx::sqlite::SqliteAutoVacuum::Full);

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(db_options)
        .await?;

    Ok(pool)
}

async fn backup(work_dir: &Path, db_file_path: &Path, backups_to_keep: u8) -> Result<(), Error> {
    if let Some(new_backup_file) =
        database::prepare_backup(work_dir, "sqlite", backups_to_keep).map_err(Error::DbBackup)?
    {
        create_backup(db_file_path, new_backup_file)
            .await
            .map_err(Error::DbBackup)?;
    }

    Ok(())
}

async fn create_backup(
    db_file_path: &Path,
    backup_file_path: PathBuf,
) -> Result<(), database::Error> {
    let pool = connect_to_db(db_file_path).await?;

    let backup_path_str = backup_file_path
        .to_str()
        .ok_or_else(|| database::Error::Database("Invalid backup path".into()))?;

    // Create a backup connection with the destination path
    let backup_options = SqliteConnectOptions::from_str(backup_path_str)
        .map_err(|e| {
            database::Error::Database(format!("Failed to create backup options: {e}").into())
        })?
        .create_if_missing(true);

    let backup_pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(backup_options)
        .await
        .map_err(|e| {
            database::Error::Database(format!("Failed to create backup connection: {e}").into())
        })?;

    // Execute backup
    sqlx::query("VACUUM INTO ?")
        .bind(backup_path_str)
        .execute(&pool)
        .await
        .map_err(|e| database::Error::Database(format!("Failed to create backup: {e}").into()))?;

    backup_pool.close().await;

    Ok(())
}

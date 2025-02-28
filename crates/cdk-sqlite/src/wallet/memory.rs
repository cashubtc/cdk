//! Wallet in memory database

use cdk_common::database::Error;

use super::WalletSqliteDatabase;

/// Creates a new in-memory [`WalletSqliteDatabase`] instance
pub async fn empty() -> Result<WalletSqliteDatabase, Error> {
    let db = WalletSqliteDatabase::new(":memory:").await?;
    db.migrate().await;
    Ok(db)
}

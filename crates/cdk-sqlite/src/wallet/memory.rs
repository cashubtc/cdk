//! Wallet in memory database

use cdk_common::database::Error;

use super::WalletSqliteDatabase;

/// Creates a new in-memory [`WalletSqliteDatabase`] instance
pub async fn empty() -> Result<WalletSqliteDatabase, Error> {
    #[cfg(not(feature = "sqlcipher"))]
    let db = WalletSqliteDatabase::new(":memory:").await?;
    #[cfg(feature = "sqlcipher")]
    let db = WalletSqliteDatabase::new(":memory:", "memory".to_owned()).await?;
    Ok(db)
}

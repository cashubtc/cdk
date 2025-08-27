//! Wallet in memory database

use cdk_common::database::Error;

use super::WalletSqliteDatabase;

/// Creates a new in-memory [`WalletSqliteDatabase`] instance
pub async fn empty() -> Result<WalletSqliteDatabase, Error> {
    #[cfg(not(feature = "sqlcipher"))]
    let path = ":memory:";

    #[cfg(feature = "sqlcipher")]
    let path = (":memory:", "memory");

    WalletSqliteDatabase::new(path).await
}

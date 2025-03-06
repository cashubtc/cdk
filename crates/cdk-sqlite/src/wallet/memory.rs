//! Wallet in memory database

use cdk_common::database::Error;

use super::WalletSqliteDatabase;

/// Creates a new in-memory [`WalletSqliteDatabase`] instance
pub async fn empty() -> Result<WalletSqliteDatabase, Error> {
    let db = WalletSqliteDatabase {
        pool: sqlx::sqlite::SqlitePool::connect(":memory:")
            .await
            .map_err(|e| Error::Database(Box::new(e)))?,
    };
    db.migrate().await;
    Ok(db)
}

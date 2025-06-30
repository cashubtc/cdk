//! SQLite Mint

use cdk_sql_base::mint::SQLMintAuthDatabase;
use cdk_sql_base::SQLMintDatabase;

mod async_rusqlite;

pub mod memory;

/// Mint SQLite implementation with rusqlite
pub type MintSqliteDatabase = SQLMintDatabase<async_rusqlite::AsyncRusqlite>;

/// Mint Auth database with rusqlite
#[cfg(feature = "auth")]
pub type MintSqliteAuthDatabase = SQLMintAuthDatabase<async_rusqlite::AsyncRusqlite>;

#[cfg(test)]
mod test {
    use cdk_common::mint_db_test;

    use super::*;

    async fn provide_db() -> MintSqliteDatabase {
        memory::empty().await.unwrap()
    }

    mint_db_test!(provide_db);
}

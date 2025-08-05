//! SQLite Mint

use cdk_sql_common::mint::SQLMintAuthDatabase;
use cdk_sql_common::SQLMintDatabase;

mod async_rusqlite;

pub mod memory;

/// Mint SQLite implementation with rusqlite
pub type MintSqliteDatabase = SQLMintDatabase<async_rusqlite::AsyncRusqlite>;

/// Mint Auth database with rusqlite
#[cfg(feature = "auth")]
pub type MintSqliteAuthDatabase = SQLMintAuthDatabase<async_rusqlite::AsyncRusqlite>;

#[cfg(test)]
mod test {
    use std::fs::remove_file;

    use cdk_common::mint_db_test;
    use cdk_sql_common::stmt::query;

    use super::*;
    use crate::mint::async_rusqlite::AsyncRusqlite;

    async fn provide_db() -> MintSqliteDatabase {
        memory::empty().await.unwrap()
    }

    mint_db_test!(provide_db);

    #[tokio::test]
    async fn open_legacy_and_migrate() {
        let file = format!(
            "{}/db.sqlite",
            std::env::temp_dir().to_str().unwrap_or_default()
        );

        {
            let _ = remove_file(&file);
            #[cfg(not(feature = "sqlcipher"))]
            let conn: AsyncRusqlite = file.as_str().into();
            #[cfg(feature = "sqlcipher")]
            let conn: AsyncRusqlite = (file.as_str(), "test".to_owned()).into();

            query(include_str!("../../tests/legacy-sqlx.sql"))
                .expect("query")
                .execute(&conn)
                .await
                .expect("create former db failed");
        }

        #[cfg(not(feature = "sqlcipher"))]
        let conn = MintSqliteDatabase::new(file.as_str()).await;

        #[cfg(feature = "sqlcipher")]
        let conn = MintSqliteDatabase::new((file.as_str(), "test".to_owned())).await;

        assert!(conn.is_ok(), "Failed with {:?}", conn.unwrap_err());

        let _ = remove_file(&file);
    }
}

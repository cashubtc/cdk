//! SQLite Mint

use cdk_sql_common::mint::SQLMintAuthDatabase;
use cdk_sql_common::SQLMintDatabase;

use crate::common::SqliteConnectionManager;

pub mod memory;

/// Mint SQLite implementation with rusqlite
pub type MintSqliteDatabase = SQLMintDatabase<SqliteConnectionManager>;

/// Mint Auth database with rusqlite
#[cfg(feature = "auth")]
pub type MintSqliteAuthDatabase = SQLMintAuthDatabase<SqliteConnectionManager>;

#[cfg(test)]
mod test {
    use std::fs::remove_file;

    use cdk_common::mint_db_test;
    use cdk_sql_common::pool::Pool;
    use cdk_sql_common::stmt::query;

    use super::*;
    use crate::common::Config;

    async fn provide_db() -> MintSqliteDatabase {
        memory::empty().await.unwrap()
    }

    mint_db_test!(provide_db);

    #[tokio::test]
    async fn test_kvstore_functionality() {
        use cdk_common::database::{MintDatabase, MintKVStoreDatabase};

        let db = provide_db().await;

        // Test basic read/write operations in transaction
        {
            let mut tx = db.begin_transaction().await.unwrap();

            // Write some test data
            tx.kv_write("test_namespace", "sub_namespace", "key1", b"value1")
                .await
                .unwrap();
            tx.kv_write("test_namespace", "sub_namespace", "key2", b"value2")
                .await
                .unwrap();
            tx.kv_write("test_namespace", "other_sub", "key3", b"value3")
                .await
                .unwrap();

            // Read back the data in the transaction
            let value1 = tx
                .kv_read("test_namespace", "sub_namespace", "key1")
                .await
                .unwrap();
            assert_eq!(value1, Some(b"value1".to_vec()));

            // List keys in namespace
            let keys = tx.kv_list("test_namespace", "sub_namespace").await.unwrap();
            assert_eq!(keys, vec!["key1", "key2"]);

            // Commit transaction
            tx.commit().await.unwrap();
        }

        // Test read operations after commit
        {
            let value1 = db
                .kv_read("test_namespace", "sub_namespace", "key1")
                .await
                .unwrap();
            assert_eq!(value1, Some(b"value1".to_vec()));

            let keys = db.kv_list("test_namespace", "sub_namespace").await.unwrap();
            assert_eq!(keys, vec!["key1", "key2"]);

            let other_keys = db.kv_list("test_namespace", "other_sub").await.unwrap();
            assert_eq!(other_keys, vec!["key3"]);
        }

        // Test update and remove operations
        {
            let mut tx = db.begin_transaction().await.unwrap();

            // Update existing key
            tx.kv_write("test_namespace", "sub_namespace", "key1", b"updated_value1")
                .await
                .unwrap();

            // Remove a key
            tx.kv_remove("test_namespace", "sub_namespace", "key2")
                .await
                .unwrap();

            tx.commit().await.unwrap();
        }

        // Verify updates
        {
            let value1 = db
                .kv_read("test_namespace", "sub_namespace", "key1")
                .await
                .unwrap();
            assert_eq!(value1, Some(b"updated_value1".to_vec()));

            let value2 = db
                .kv_read("test_namespace", "sub_namespace", "key2")
                .await
                .unwrap();
            assert_eq!(value2, None);

            let keys = db.kv_list("test_namespace", "sub_namespace").await.unwrap();
            assert_eq!(keys, vec!["key1"]);
        }
    }

    #[tokio::test]
    async fn open_legacy_and_migrate() {
        let file = format!(
            "{}/db.sqlite",
            std::env::temp_dir().to_str().unwrap_or_default()
        );

        {
            let _ = remove_file(&file);
            #[cfg(not(feature = "sqlcipher"))]
            let config: Config = file.as_str().into();
            #[cfg(feature = "sqlcipher")]
            let config: Config = (file.as_str(), "test").into();

            let pool = Pool::<SqliteConnectionManager>::new(config);

            let conn = pool.get().expect("valid connection");

            query(include_str!("../../tests/legacy-sqlx.sql"))
                .expect("query")
                .execute(&*conn)
                .await
                .expect("create former db failed");
        }

        #[cfg(not(feature = "sqlcipher"))]
        let conn = MintSqliteDatabase::new(file.as_str()).await;

        #[cfg(feature = "sqlcipher")]
        let conn = MintSqliteDatabase::new((file.as_str(), "test")).await;

        assert!(conn.is_ok(), "Failed with {:?}", conn.unwrap_err());

        let _ = remove_file(&file);
    }
}

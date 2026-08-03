//! SQLite Mint

use cdk_sql_common::mint::SQLMintAuthDatabase;
use cdk_sql_common::SQLMintDatabase;

use crate::common::SqliteConnectionManager;

pub mod memory;

/// Mint SQLite implementation with rusqlite
pub type MintSqliteDatabase = SQLMintDatabase<SqliteConnectionManager>;

/// Mint Auth database with rusqlite
pub type MintSqliteAuthDatabase = SQLMintAuthDatabase<SqliteConnectionManager>;

#[cfg(test)]
mod test {
    use std::fs::remove_file;
    use std::time::Duration;

    use cdk_common::mint_db_test;
    use cdk_common::pub_sub::test::CustomPubSub;
    use cdk_common::pub_sub::{Pubsub, Spec};
    use cdk_sql_common::mint::bus::{SqlBusConnector, SqlBusOptions};
    use cdk_sql_common::pool::Pool;
    use cdk_sql_common::stmt::query;

    use super::*;
    use crate::common::Config;

    async fn provide_db(_test_name: String) -> MintSqliteDatabase {
        memory::empty().await.unwrap()
    }

    mint_db_test!(provide_db);

    /// Short intervals so the poll-based tests finish quickly.
    fn fast_bus_options() -> SqlBusOptions {
        SqlBusOptions {
            poll_interval: Duration::from_millis(25),
            retention: Duration::from_secs(3600),
        }
    }

    /// Wrap a database in a `Pubsub` driven by the SQL polling bus.
    async fn sql_bus_pubsub(db: &MintSqliteDatabase) -> Pubsub<CustomPubSub> {
        let connector = SqlBusConnector::connect(db.pool(), fast_bus_options())
            .await
            .expect("connect sql bus");
        Pubsub::new_with_bus(CustomPubSub::new_instance(()), move |local| {
            connector.build(local)
        })
    }

    /// Factory for the generic bus suite: one SQL-backed node per test.
    async fn provide_sql_bus(_test_id: String) -> Pubsub<CustomPubSub> {
        let db = memory::empty().await.expect("in-memory db");
        // The database Arc is dropped here, but the bus keeps the connection
        // pool (and thus the in-memory database) alive through its own clone.
        sql_bus_pubsub(&db).await
    }

    cdk_common::bus_test!(provide_sql_bus);

    /// Store factory for the shared cross-instance suite: a temp-file SQLite DB
    /// (unique per test) whose pool is shared by the two logical instances the
    /// suite builds. A `:memory:` pool cannot be used here: it is
    /// single-connection and per-connection, so two instances would not see the
    /// same store.
    async fn provide_sql_bus_store(
        test_id: String,
    ) -> std::sync::Arc<Pool<SqliteConnectionManager>> {
        let file = format!(
            "{}/cdk_sql_bus_{test_id}.sqlite",
            std::env::temp_dir().to_str().unwrap_or_default()
        );
        MintSqliteDatabase::new(file.as_str())
            .await
            .expect("db")
            .pool()
    }

    cdk_sql_common::sql_bus_test!(provide_sql_bus_store);

    #[tokio::test]
    async fn bug_opening_relative_path() {
        let config: Config = "test.db".into();

        let pool = Pool::<SqliteConnectionManager>::new(config);
        let db = pool.get().await;
        assert!(db.is_ok());
        let _ = remove_file("test.db");
    }

    #[tokio::test]
    async fn exhausted_in_memory_pool_times_out() {
        let config: Config = ":memory:".into();
        let pool = Pool::<SqliteConnectionManager>::new(config);

        let _conn = pool.get().await.expect("valid connection");
        let result = pool.get_timeout(Duration::from_millis(10)).await;

        assert!(matches!(result, Err(cdk_sql_common::pool::Error::Timeout)));
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

            let conn = pool.get().await.expect("valid connection");

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

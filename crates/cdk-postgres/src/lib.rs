//! PostgreSQL storage backed by Deadpool.

mod backend;
mod config;
mod connection;
mod db;
mod tls;
mod value;

pub use self::backend::PostgresBackend;
pub use self::config::PgConfig;
pub use self::connection::{PostgresConnection, PostgresTransaction};

#[cfg(feature = "mint")]
/// Mint database backed by PostgreSQL.
pub type MintPgDatabase = cdk_sql_common::SQLMintDatabase<PostgresBackend>;
#[cfg(feature = "mint")]
/// Mint authentication database backed by PostgreSQL.
pub type MintPgAuthDatabase = cdk_sql_common::mint::SQLMintAuthDatabase<PostgresBackend>;
#[cfg(feature = "wallet")]
/// Wallet database backed by PostgreSQL.
pub type WalletPgDatabase = cdk_sql_common::SQLWalletDatabase<PostgresBackend>;

#[cfg(feature = "wallet")]
/// Create a PostgreSQL wallet database and apply migrations.
pub async fn new_wallet_pg_database(
    conn_str: &str,
) -> Result<WalletPgDatabase, cdk_common::database::Error> {
    WalletPgDatabase::new(conn_str).await
}

#[cfg(all(test, feature = "mint", feature = "wallet"))]
mod test {
    use std::sync::Arc;

    use cdk_common::{mint_db_test, wallet_db_test, QuoteId};

    use super::*;

    async fn provide_mint_db(test_id: String) -> MintPgDatabase {
        let db_url = std::env::var("CDK_MINTD_DATABASE_URL")
            .or_else(|_| std::env::var("PG_DB_URL")) // Fallback for compatibility
            .unwrap_or(
                "host=localhost user=cdk_user password=cdk_password dbname=cdk_mint port=5432"
                    .to_owned(),
            );

        let db_url = format!("{db_url} schema={test_id}");

        MintPgDatabase::new(db_url.as_str())
            .await
            .expect("database")
    }

    mint_db_test!(provide_mint_db);

    #[tokio::test]
    async fn mint_pool_accepts_single_connection_configuration() {
        use cdk_common::database::MintDatabase;

        let test_id = format!("test_single_connection_pool_{}", uuid::Uuid::new_v4());
        let db_url = std::env::var("CDK_MINTD_DATABASE_URL")
            .or_else(|_| std::env::var("PG_DB_URL"))
            .unwrap_or(
                "host=localhost user=cdk_user password=cdk_password dbname=cdk_mint port=5432"
                    .to_owned(),
            );
        let config = PgConfig::new(
            &format!("{db_url} schema={test_id}"),
            None,
            Some(1),
            Some(10),
        );

        let db = MintPgDatabase::new(config)
            .await
            .expect("single-connection mint pool should remain supported");
        let regular = MintDatabase::begin_transaction(&db)
            .await
            .expect("regular transaction");
        regular.rollback().await.expect("regular rollback");
    }

    #[tokio::test]
    async fn quote_lock_batch_excludes_concurrent_transaction() {
        use std::sync::Arc;
        use std::time::Duration;

        use cdk_common::database::MintDatabase;

        let test_id = format!("test_quote_lock_batch_{}", uuid::Uuid::new_v4());
        let db = Arc::new(provide_mint_db(test_id).await);
        let first = QuoteId::new();
        let second = QuoteId::new();

        let mut holder = MintDatabase::begin_transaction(&*db).await.expect("tx");
        holder
            .lock_quotes(&[first.clone(), second.clone()])
            .await
            .expect("lock");

        let waiter = tokio::spawn({
            let db = db.clone();
            async move {
                let mut tx = MintDatabase::begin_transaction(&*db).await.expect("tx");
                tx.lock_quotes(&[second, first]).await.expect("lock");
                tx.commit().await.expect("commit");
            }
        });

        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            !waiter.is_finished(),
            "reversed quote batch did not wait for the holder"
        );

        holder.commit().await.expect("commit");
        tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .expect("reversed quote batch remained blocked")
            .expect("waiter task");
    }

    #[tokio::test]
    async fn kvstore_compare_and_swap() {
        let test_id = format!("test_kvstore_compare_and_swap_{}", uuid::Uuid::new_v4());
        cdk_common::database::mint::test::kvstore_compare_and_swap(provide_mint_db(test_id).await)
            .await;
    }

    #[tokio::test]
    async fn concurrent_mint_quote_batches_use_consistent_lock_order() {
        let test_id = format!(
            "test_concurrent_mint_quote_batches_{}",
            uuid::Uuid::new_v4()
        );
        cdk_common::database::mint::test::concurrent_mint_quote_batches_use_consistent_lock_order(
            Arc::new(provide_mint_db(test_id).await),
        )
        .await;
    }

    #[tokio::test]
    async fn concurrent_multi_keyset_spends_use_consistent_lock_order() {
        let test_id = format!(
            "test_concurrent_multi_keyset_spends_{}",
            uuid::Uuid::new_v4()
        );
        cdk_common::database::mint::test::concurrent_multi_keyset_spends_use_consistent_lock_order(
            Arc::new(provide_mint_db(test_id).await),
        )
        .await;
    }

    async fn provide_wallet_db(test_id: String) -> WalletPgDatabase {
        let db_url = std::env::var("CDK_MINTD_DATABASE_URL")
            .or_else(|_| std::env::var("PG_DB_URL")) // Fallback for compatibility
            .unwrap_or(
                "host=localhost user=cdk_user password=cdk_password dbname=cdk_mint port=5432"
                    .to_owned(),
            );

        let db_url = format!("{db_url} schema={test_id}");

        WalletPgDatabase::new(db_url.as_str())
            .await
            .expect("database")
    }

    wallet_db_test!(provide_wallet_db);

    #[test]
    fn pgconfig_debug_does_not_leak_password() {
        let config = PgConfig::from("host=localhost user=u password=hunter2secret dbname=d");
        let rendered = format!("{config:?}");

        assert!(
            !rendered.contains("hunter2secret"),
            "PgConfig Debug leaked the DB password: {rendered}"
        );
    }
}

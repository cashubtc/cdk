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
    use std::path::Path;
    use std::str::FromStr;
    use std::sync::Arc;
    use std::time::Duration;

    use cdk_common::database::{self, MintAuthDatabase};
    use cdk_common::secret::Secret;
    use cdk_common::{mint_db_test, AuthProof, Id, SecretKey, State};
    use cdk_sql_common::pool::Pool;
    use cdk_sql_common::stmt::query;

    use super::*;
    use crate::common::Config;

    async fn provide_db(_test_name: String) -> MintSqliteDatabase {
        memory::empty().await.unwrap()
    }

    fn migration_test_config(path: &Path) -> Config {
        #[cfg(not(feature = "sqlcipher"))]
        {
            path.to_path_buf().into()
        }
        #[cfg(feature = "sqlcipher")]
        {
            (path.to_path_buf(), "test".to_owned()).into()
        }
    }

    async fn migration_query_succeeds(path: &Path, sql: &str) -> bool {
        let pool = Pool::<SqliteConnectionManager>::new(migration_test_config(path));
        let conn = pool.get().await.expect("valid migration test connection");
        query(sql)
            .expect("valid migration test query")
            .fetch_all(&*conn)
            .await
            .is_ok()
    }

    mint_db_test!(provide_db);

    #[tokio::test]
    async fn kvstore_compare_and_swap() {
        cdk_common::database::mint::test::kvstore_compare_and_swap(
            provide_db("test_kvstore_compare_and_swap".to_owned()).await,
        )
        .await;
    }

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

    async fn spend_auth_proof(
        db: Arc<MintSqliteAuthDatabase>,
        proof: AuthProof,
    ) -> Result<(), database::Error> {
        let mut tx = db.as_ref().begin_transaction().await?;
        tx.add_proof(proof).await?;
        tx.commit().await
    }

    #[tokio::test]
    async fn concurrent_blind_auth_proof_spend_allows_one_request() {
        let path = std::env::temp_dir().join(format!(
            "cdk-blind-auth-replay-{}.sqlite",
            uuid::Uuid::new_v4()
        ));

        #[cfg(not(feature = "sqlcipher"))]
        let db = Arc::new(
            MintSqliteAuthDatabase::new(&path)
                .await
                .expect("auth database"),
        );
        #[cfg(feature = "sqlcipher")]
        let db = Arc::new(
            MintSqliteAuthDatabase::new((path.clone(), "test".to_owned()))
                .await
                .expect("auth database"),
        );

        let proof = AuthProof {
            keyset_id: Id::from_str("00916bbf7ef91a36").expect("valid keyset id"),
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            dleq: None,
        };
        let y = proof.y().expect("proof y");

        let (first, second) = tokio::join!(
            spend_auth_proof(db.clone(), proof.clone()),
            spend_auth_proof(db.clone(), proof)
        );

        assert!(matches!(
            (&first, &second),
            (Ok(()), Err(database::Error::Duplicate)) | (Err(database::Error::Duplicate), Ok(()))
        ));
        assert_eq!(
            db.get_proofs_states(&[y]).await.expect("proof state"),
            vec![Some(State::Spent)]
        );

        drop(db);
        remove_file(path).expect("remove auth database");
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

    #[tokio::test]
    async fn backward_migrations_are_preflighted_and_can_be_reapplied() {
        let path = std::env::temp_dir().join(format!(
            "cdk-mint-backward-migrations-{}.sqlite",
            uuid::Uuid::new_v4()
        ));

        let db = MintSqliteDatabase::new(migration_test_config(&path))
            .await
            .expect("apply forward migrations");
        drop(db);

        let error = MintSqliteDatabase::rollback(migration_test_config(&path), 4)
            .await
            .expect_err("rollback must stop before an irreversible migration");
        assert!(
            error
                .to_string()
                .contains("20260520120000_rename_selected_estimated_blocks_to_fee_index.sql"),
            "unexpected rollback error: {error}"
        );
        assert!(
            migration_query_succeeds(&path, "SELECT last_checked FROM mint_quote LIMIT 1").await,
            "preflight failure must not roll back the newest migration"
        );
        assert!(
            migration_query_succeeds(&path, "SELECT epoch FROM keyset_epoch LIMIT 1").await,
            "preflight failure must not roll back earlier migrations"
        );

        let rolled_back = MintSqliteDatabase::rollback(migration_test_config(&path), 3)
            .await
            .expect("roll back reversible migrations");
        assert_eq!(
            rolled_back,
            vec![
                "20260811000000_add_last_checked_to_mint_quote.sql",
                "20260728000000_add_keyset_epoch.sql",
                "20260630000000_add_updated_at_to_mint_quote.sql",
            ]
        );
        assert!(
            !migration_query_succeeds(&path, "SELECT last_checked FROM mint_quote LIMIT 1").await
        );
        assert!(!migration_query_succeeds(&path, "SELECT epoch FROM keyset_epoch LIMIT 1").await);
        assert!(
            !migration_query_succeeds(&path, "SELECT updated_at FROM mint_quote LIMIT 1").await
        );

        let db = MintSqliteDatabase::new(migration_test_config(&path))
            .await
            .expect("reapply rolled-back migrations");
        drop(db);
        assert!(
            migration_query_succeeds(&path, "SELECT last_checked FROM mint_quote LIMIT 1").await
        );
        assert!(migration_query_succeeds(&path, "SELECT epoch FROM keyset_epoch LIMIT 1").await);
        assert!(migration_query_succeeds(&path, "SELECT updated_at FROM mint_quote LIMIT 1").await);

        remove_file(path).expect("remove migration test database");
    }
}

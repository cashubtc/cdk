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
    use std::str::FromStr;
    use std::sync::Arc;
    use std::time::Duration;

    use cdk_common::database::{self, MintAuthDatabase};
    use cdk_common::secret::Secret;
    use cdk_common::{mint_db_test, AuthProof, Id, SecretKey, State};
    use cdk_sql_common::database::ConnectionWithTransaction;
    use cdk_sql_common::pool::Pool;
    use cdk_sql_common::stmt::query;

    use super::*;
    use crate::common::Config;

    async fn provide_db(_test_name: String) -> MintSqliteDatabase {
        memory::empty().await.unwrap()
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

    #[tokio::test]
    async fn cloned_config_creates_independent_in_memory_databases() {
        let config: Config = ":memory:".into();
        let _first = MintSqliteDatabase::new(config.clone())
            .await
            .expect("first database");
        let _second = MintSqliteDatabase::new(config)
            .await
            .expect("independent second database");
    }

    #[test]
    fn discarded_in_memory_connection_is_not_replaced() {
        let config: Config = ":memory:".into();
        let pool = Pool::<SqliteConnectionManager>::new(config);
        let runtime = || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime")
        };

        // Dropping the runtime drops the unpolled rollback task, which discards
        // the connection and with it the in-memory database.
        let first = runtime();
        first.block_on(async {
            let conn = pool.get().await.expect("valid connection");
            let tx = ConnectionWithTransaction::new(conn)
                .await
                .expect("transaction");
            drop(tx);
        });
        drop(first);

        runtime().block_on(async {
            assert!(matches!(
                pool.get().await,
                Err(cdk_sql_common::pool::Error::Resource(_))
            ));
        });
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

    /// The mint column carries the same requirement as the wallet one: a stored
    /// zero reads back as an expiry at the epoch on a build that still types the
    /// field as `Option<u64>`, which makes the keyset unusable.
    #[tokio::test]
    async fn zero_final_expiry_is_stored_as_null() {
        use cdk_common::bitcoin::bip32::DerivationPath;
        use cdk_common::database::mint::KeysDatabase;
        use cdk_common::mint::MintKeySetInfo;
        use cdk_common::nuts::CurrencyUnit;
        use cdk_sql_common::value::Value;

        use crate::test_support::stored_value;

        let path = std::env::temp_dir()
            .join(format!(
                "cdk-test-mint-null-expiry-{}.sqlite",
                uuid::Uuid::new_v4()
            ))
            .to_str()
            .expect("temp dir is valid UTF-8")
            .to_owned();

        #[cfg(feature = "sqlcipher")]
        let db = MintSqliteDatabase::new((path.as_str(), "test"))
            .await
            .expect("mint database");

        #[cfg(not(feature = "sqlcipher"))]
        let db = MintSqliteDatabase::new(path.as_str())
            .await
            .expect("mint database");

        let mut keyset_info = MintKeySetInfo {
            id: Id::from_str("00916bbf7ef91a36").expect("keyset id"),
            unit: CurrencyUnit::Sat,
            active: true,
            valid_from: 0,
            final_expiry: 0,
            derivation_path: DerivationPath::from_str("m/0'/0'/0'").expect("derivation path"),
            derivation_path_index: Some(0),
            input_fee_ppk: 0,
            amounts: vec![1, 2, 4, 8],
            issuer_version: None,
        };

        let mut tx = KeysDatabase::begin_transaction(&db).await.expect("tx");
        tx.add_keyset_info(keyset_info.clone()).await.expect("add");
        tx.commit().await.expect("commit");
        assert_eq!(
            stored_value(&path, "SELECT valid_to FROM keyset").await,
            Value::Null
        );

        keyset_info.final_expiry = 1_900_000_000;
        let mut tx = KeysDatabase::begin_transaction(&db).await.expect("tx");
        tx.add_keyset_info(keyset_info.clone()).await.expect("add");
        tx.commit().await.expect("commit");
        assert_eq!(
            stored_value(&path, "SELECT valid_to FROM keyset").await,
            Value::Integer(1_900_000_000)
        );

        keyset_info.final_expiry = 0;
        let mut tx = KeysDatabase::begin_transaction(&db).await.expect("tx");
        tx.add_keyset_info(keyset_info).await.expect("add");
        tx.commit().await.expect("commit");
        assert_eq!(
            stored_value(&path, "SELECT valid_to FROM keyset").await,
            Value::Null
        );

        drop(db);
        let _ = remove_file(&path);
    }

    /// A database written before zero meant "no expiry" can hold a literal zero
    /// in the column, which an older build reads as an expiry at the epoch, so
    /// opening it has to rewrite those rows to NULL.
    #[tokio::test]
    async fn migration_nulls_a_stored_zero_valid_to() {
        use cdk_sql_common::value::Value;

        use crate::test_support::stored_value;

        let file = std::env::temp_dir()
            .join(format!(
                "cdk-test-zero-valid-to-{}.sqlite",
                uuid::Uuid::new_v4()
            ))
            .to_str()
            .expect("temp dir is valid UTF-8")
            .to_owned();

        {
            #[cfg(not(feature = "sqlcipher"))]
            let config: Config = file.as_str().into();
            #[cfg(feature = "sqlcipher")]
            let config: Config = (file.as_str(), "test").into();

            let pool = Pool::<SqliteConnectionManager>::new(config);
            let conn = pool.get().await.expect("valid connection");

            query(include_str!("../../tests/legacy-sqlx.sql"))
                .expect("query")
                .batch(&*conn)
                .await
                .expect("create former db failed");

            for (id, valid_to, index) in [
                ("00916bbf7ef91a36", "0", 1),
                ("00916bbf7ef91a37", "1900000000", 2),
            ] {
                query(&format!(
                    "INSERT INTO keyset VALUES ('{id}', 'sat', 1, 1749844864, {valid_to}, 'm/0/0/0', 32, 0, {index})"
                ))
                .expect("query")
                .execute(&*conn)
                .await
                .expect("seed legacy expiry");
            }
        }

        #[cfg(feature = "sqlcipher")]
        let db = MintSqliteDatabase::new((file.as_str(), "test"))
            .await
            .expect("mint database");

        #[cfg(not(feature = "sqlcipher"))]
        let db = MintSqliteDatabase::new(file.as_str())
            .await
            .expect("mint database");

        assert_eq!(
            stored_value(
                &file,
                "SELECT valid_to FROM keyset WHERE id = '00916bbf7ef91a36'"
            )
            .await,
            Value::Null
        );
        assert_eq!(
            stored_value(
                &file,
                "SELECT valid_to FROM keyset WHERE id = '00916bbf7ef91a37'"
            )
            .await,
            Value::Integer(1_900_000_000)
        );

        drop(db);
        let _ = remove_file(&file);
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

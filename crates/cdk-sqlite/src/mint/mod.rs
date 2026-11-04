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

    #[cfg(not(feature = "sqlcipher"))]
    #[tokio::test]
    async fn rerandomization_migration_preserves_existing_proofs_and_reopens() {
        use cdk_common::database::{MintProofsDatabase, MintSignaturesDatabase};
        let path = std::env::temp_dir().join(format!(
            "cdk-rerandomization-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let db = MintSqliteDatabase::new(&path).await.unwrap();
        drop(db);
        let key = SecretKey::generate().public_key();
        let proof = cdk_common::Proof {
            amount: cdk_common::Amount::ONE,
            keyset_id: Id::from_str("00916bbf7ef91a36").unwrap(),
            secret: Secret::generate(),
            c: key,
            witness: None,
            dleq: None,
            p2pk_e: None,
        };
        let y = proof.y().unwrap();
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            // Recreate the immediately preceding schema with an existing spent proof.
            conn.execute_batch("ALTER TABLE proof DROP COLUMN rerandomization_r; DELETE FROM migrations WHERE name = '20261014000000_add_rerandomization_r.sql';").unwrap();
            conn.execute("INSERT INTO proof (y, amount, keyset_id, secret, c, state) VALUES (?1, 1, ?2, ?3, ?4, 'SPENT')", rusqlite::params![y.to_bytes().to_vec(), proof.keyset_id.to_string(), proof.secret.to_string(), key.to_bytes().to_vec()]).unwrap();
        }
        let db = MintSqliteDatabase::new(&path).await.unwrap();
        assert_eq!(db.get_proofs_by_ys(&[y]).await.unwrap(), vec![Some(proof)]);
        assert!(db.get_rerandomization_r(&[y]).await.unwrap().is_empty());
        assert!(db.get_blind_signatures(&[key]).await.unwrap()[0].is_none());
        drop(db);
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            assert!(conn
                .execute("UPDATE proof SET rerandomization_r = ?1", [vec![42u8; 31]])
                .is_err());
            conn.execute("UPDATE proof SET rerandomization_r = ?1", [vec![42u8; 32]])
                .unwrap();
            assert!(conn
                .execute("UPDATE proof SET state = 'PENDING'", [])
                .is_err());
            // Old named-column readers tolerate the additional column.
            let state: String = conn
                .query_row(
                    "SELECT state FROM proof WHERE y = ?1",
                    [y.to_bytes().to_vec()],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(state, "SPENT");
        }
        let db = MintSqliteDatabase::new(&path).await.unwrap();
        assert_eq!(
            db.get_rerandomization_r(&[y]).await.unwrap().get(&y),
            Some(&[42; 32])
        );
        drop(db);
        remove_file(path).unwrap();
    }

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

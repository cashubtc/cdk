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
    async fn reconciliation_raises_issued_to_cover_what_a_keyset_owes() {
        use cdk_common::database::{MintDatabase, MintProofsDatabase, MintSignaturesDatabase};
        use cdk_common::Amount;

        let path = std::env::temp_dir().join(format!(
            "cdk-reconcile-{}.db",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock before the unix epoch")
                .as_nanos()
        ));
        let config: Config = path.to_str().expect("non utf8 temp dir").into();
        let keyset_id = Id::from_str("00916bbf7ef91a36").unwrap();

        let db = MintSqliteDatabase::new(config.clone()).await.unwrap();

        let fee_collected = || async {
            let pool = Pool::<SqliteConnectionManager>::new(config.clone());
            let conn = pool.get().await.unwrap();
            query(r#"SELECT fee_collected FROM keyset_amounts WHERE keyset_id = :keyset_id"#)
                .unwrap()
                .bind("keyset_id", keyset_id.to_string())
                .pluck(&*conn)
                .await
                .unwrap()
        };

        {
            let pool = Pool::<SqliteConnectionManager>::new(config.clone());
            let conn = pool.get().await.unwrap();
            query(
                r#"
                INSERT INTO keyset_amounts
                    (keyset_id, total_issued, total_redeemed, total_reserved, fee_collected)
                VALUES (:keyset_id, 10, 70, 30, 7)
                "#,
            )
            .unwrap()
            .bind("keyset_id", keyset_id.to_string())
            .execute(&*conn)
            .await
            .unwrap();
        }

        // Opening the database must not repair on its own; only an explicit
        // reconcile may move the counters.
        assert_eq!(
            db.get_total_issued()
                .await
                .unwrap()
                .get(&keyset_id)
                .copied(),
            Some(Amount::from(10)),
            "opening the database must leave accounting alone"
        );

        db.reconcile_keyset_ledger().await.unwrap();

        assert_eq!(
            db.get_total_issued()
                .await
                .unwrap()
                .get(&keyset_id)
                .copied(),
            Some(Amount::from(100)),
            "issued is raised to what the keyset already owes"
        );
        assert_eq!(
            db.get_total_redeemed()
                .await
                .unwrap()
                .get(&keyset_id)
                .copied(),
            Some(Amount::from(70)),
            "the debits are left alone"
        );
        assert_eq!(
            db.get_total_reserved()
                .await
                .unwrap()
                .get(&keyset_id)
                .copied(),
            Some(Amount::from(30))
        );
        assert!(
            matches!(
                fee_collected().await,
                Some(cdk_sql_common::value::Value::Integer(7))
            ),
            "reconciliation must not touch collected fees"
        );

        // A second pass has nothing left to clamp and must change nothing.
        db.reconcile_keyset_ledger().await.unwrap();
        assert_eq!(
            db.get_total_issued()
                .await
                .unwrap()
                .get(&keyset_id)
                .copied(),
            Some(Amount::from(100)),
            "reconciliation is idempotent"
        );

        drop(db);
        let _ = remove_file(&path);
    }

    #[tokio::test]
    async fn burning_proofs_survives_a_reservation_that_drifted_low() {
        use cdk_common::database::{MintDatabase, MintProofsDatabase};
        use cdk_common::mint::Operation;
        use cdk_common::{Amount, Proof};

        let path = std::env::temp_dir().join(format!(
            "cdk-drift-{}.db",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock before the unix epoch")
                .as_nanos()
        ));
        let config: Config = path.to_str().expect("non utf8 temp dir").into();
        let keyset_id = Id::from_str("00916bbf7ef91a36").unwrap();

        let db = MintSqliteDatabase::new(config.clone()).await.unwrap();

        let raw = |sql: &'static str| {
            let config = config.clone();
            async move {
                let pool = Pool::<SqliteConnectionManager>::new(config);
                let conn = pool.get().await.unwrap();
                query(sql)
                    .unwrap()
                    .bind("keyset_id", keyset_id.to_string())
                    .execute(&*conn)
                    .await
                    .unwrap();
            }
        };

        raw(r#"
            INSERT INTO keyset_amounts
                (keyset_id, total_issued, total_redeemed, total_reserved)
            VALUES (:keyset_id, 100, 0, 0)
            "#)
        .await;

        let proofs = vec![Proof {
            amount: Amount::from(100),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
            p2pk_e: None,
        }];
        let ys: Vec<_> = proofs.iter().map(|p| p.y().unwrap()).collect();

        let mut tx = MintDatabase::begin_transaction(&db).await.unwrap();
        tx.add_proofs(
            proofs,
            None,
            &Operation::new_swap(Amount::ZERO, Amount::ZERO, Amount::ZERO),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();

        // Stand in for a keyset imported from another implementation, or one
        // restored from a partial backup: the mint holds the proofs but its
        // counters do not say so.
        raw(r#"
            UPDATE keyset_amounts
            SET total_issued = 0, total_reserved = 0
            WHERE keyset_id = :keyset_id
            "#)
        .await;

        let mut tx = MintDatabase::begin_transaction(&db).await.unwrap();
        let mut records = tx.get_proofs(&ys).await.unwrap();
        tx.update_proofs_state(&mut records, State::Spent)
            .await
            .expect("burning proofs the mint holds must never be refused");
        tx.commit().await.unwrap();

        assert_eq!(
            db.get_total_redeemed()
                .await
                .unwrap()
                .get(&keyset_id)
                .copied(),
            Some(Amount::from(100)),
            "the burn is recorded even though it raises what the keyset owes"
        );
        assert_eq!(
            db.get_total_reserved()
                .await
                .unwrap()
                .get(&keyset_id)
                .copied(),
            Some(Amount::ZERO),
            "a reservation that had drifted low clamps instead of going negative"
        );

        drop(db);
        let _ = remove_file(&path);
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
}

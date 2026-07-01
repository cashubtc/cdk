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
    use cdk_sql_common::pool::Pool;
    use cdk_sql_common::stmt::query;

    use super::*;
    use crate::common::Config;

    async fn provide_db(_test_name: String) -> MintSqliteDatabase {
        memory::empty().await.unwrap()
    }

    mint_db_test!(provide_db);

    /// End-to-end check that the transparency event log (see
    /// `docs/adr/0001-append-only-transparency-log.md`) is actually
    /// populated by real mutations, not just present as an unused table.
    #[tokio::test]
    async fn transparency_log_is_populated_by_real_mutations() {
        use std::str::FromStr;

        use bitcoin::bip32::DerivationPath;
        use cdk_common::common::IssuerVersion;
        use cdk_common::database::mint::{
            Database as MintDatabase, EventOp, KeysDatabase, LoggedEntity, TransparencyLogDatabase,
        };
        use cdk_common::mint::{MintKeySetInfo, Operation};
        use cdk_common::nuts::State;
        use cdk_common::secret::Secret;
        use cdk_common::state::check_state_transition;
        use cdk_common::{Amount, CurrencyUnit, Id, Proof, SecretKey};

        let db = memory::empty().await.unwrap();

        assert_eq!(
            TransparencyLogDatabase::latest_event_log_seq(&db)
                .await
                .unwrap(),
            0,
            "a fresh mint has an empty transparency log"
        );

        // 1. `set_active_keyset` should log a Keyset/Update entry.
        let keyset_id = Id::from_str("00916bbf7ef91a36").unwrap();
        let keyset_info = MintKeySetInfo {
            id: keyset_id,
            unit: CurrencyUnit::Sat,
            active: false,
            valid_from: 0,
            final_expiry: None,
            derivation_path: DerivationPath::from_str("m/0'/0'/0'").unwrap(),
            derivation_path_index: Some(0),
            input_fee_ppk: 0,
            amounts: vec![1, 2, 4, 8],
            issuer_version: IssuerVersion::from_str("cdk/0.1.0").ok(),
        };
        let mut tx = KeysDatabase::begin_transaction(&db).await.unwrap();
        tx.add_keyset_info(keyset_info).await.unwrap();
        tx.set_active_keyset(CurrencyUnit::Sat, keyset_id)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        // 2. Add and then update a proof's state -> Proof/Update entry.
        let proof = Proof {
            amount: Amount::from(4),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
            p2pk_e: None,
        };
        let y = proof.y().unwrap();
        let mut tx = MintDatabase::begin_transaction(&db).await.unwrap();
        tx.add_proofs(
            vec![proof],
            None,
            &Operation::new_swap(Amount::ZERO, Amount::ZERO, Amount::ZERO),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();

        let mut tx = MintDatabase::begin_transaction(&db).await.unwrap();
        let mut proofs = tx.get_proofs(&[y]).await.unwrap();
        check_state_transition(proofs.state, State::Pending).unwrap();
        tx.update_proofs_state(&mut proofs, State::Pending)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let seq_after = TransparencyLogDatabase::latest_event_log_seq(&db)
            .await
            .unwrap();
        assert!(
            seq_after >= 2,
            "expected at least the keyset and proof updates to be logged, got seq {seq_after}"
        );

        let entries = TransparencyLogDatabase::get_event_log_range(&db, 1, seq_after + 1)
            .await
            .unwrap();
        assert_eq!(entries.len() as u64, seq_after);

        assert!(
            entries
                .iter()
                .any(|e| e.entity_type == LoggedEntity::Keyset && e.op == EventOp::Update),
            "missing keyset activation log entry: {entries:?}"
        );
        assert!(
            entries.iter().any(|e| e.entity_type == LoggedEntity::Proof
                && e.entity_id == y.to_string()
                && e.op == EventOp::Update),
            "missing proof state-update log entry: {entries:?}"
        );

        // Every stored leaf_hash must be independently reproducible from
        // the entry's own fields — this is the property external
        // playback/verification tooling depends on.
        for entry in &entries {
            let expected = cdk_tlog::merkle::leaf_hash(&entry.leaf_preimage());
            assert_eq!(
                entry.leaf_hash, expected,
                "stored leaf_hash must match a fresh computation from the entry's own fields"
            );
        }

        // seq must be strictly increasing in returned order.
        for pair in entries.windows(2) {
            assert!(pair[0].seq < pair[1].seq);
        }
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

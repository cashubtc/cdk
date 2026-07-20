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
    #[cfg(feature = "federation")]
    use std::collections::{BTreeMap, BTreeSet};
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
    #[cfg(feature = "federation")]
    use cdk_sql_common::value::Value;

    use super::*;
    use crate::common::Config;

    async fn provide_db(_test_name: String) -> MintSqliteDatabase {
        memory::empty().await.unwrap()
    }

    #[cfg(not(feature = "sqlcipher"))]
    fn melt_quote_schema(connection: &rusqlite::Connection) -> Vec<(String, String, String)> {
        let mut statement = connection
            .prepare(
                r#"
                SELECT type, name, sql
                FROM sqlite_master
                WHERE tbl_name = 'melt_quote'
                  AND sql IS NOT NULL
                ORDER BY type, name
                "#,
            )
            .expect("prepare melt quote schema query");
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("query melt quote schema")
            .collect::<Result<Vec<_>, _>>()
            .expect("read melt quote schema")
    }

    mint_db_test!(provide_db);

    #[cfg(not(feature = "sqlcipher"))]
    #[tokio::test]
    async fn failed_state_migration_preserves_pre_federation_database_and_matches_fresh_schema() {
        let upgraded = rusqlite::Connection::open_in_memory().expect("upgrade database");
        upgraded
            .execute_batch(
                r#"
                PRAGMA foreign_keys = ON;

                CREATE TABLE melt_quote (
                    id TEXT PRIMARY KEY,
                    unit TEXT NOT NULL,
                    amount INTEGER NOT NULL,
                    request TEXT NOT NULL,
                    fee_reserve INTEGER NOT NULL,
                    expiry INTEGER NOT NULL,
                    state TEXT CHECK (
                        state IN ('UNPAID', 'PENDING', 'PAID')
                    ) NOT NULL DEFAULT 'UNPAID',
                    payment_proof TEXT,
                    request_lookup_id TEXT,
                    created_time INTEGER NOT NULL DEFAULT 0,
                    paid_time INTEGER,
                    payment_method TEXT NOT NULL DEFAULT 'bolt11',
                    options TEXT,
                    request_lookup_id_kind TEXT,
                    estimated_blocks INTEGER,
                    fee_options TEXT,
                    extra_json TEXT,
                    selected_fee_index INTEGER
                );

                CREATE INDEX melt_quote_state_index ON melt_quote(state);
                CREATE INDEX idx_melt_quote_request_lookup_id ON melt_quote(request_lookup_id);
                CREATE INDEX idx_melt_quote_request_lookup_id_and_kind
                ON melt_quote(request_lookup_id, request_lookup_id_kind);
                CREATE UNIQUE INDEX unique_pending_paid_lookup_id
                ON melt_quote(request_lookup_id)
                WHERE state IN ('PENDING', 'PAID');

                CREATE TABLE melt_request (
                    quote_id TEXT PRIMARY KEY,
                    inputs_amount INTEGER NOT NULL,
                    inputs_fee INTEGER NOT NULL,
                    FOREIGN KEY (quote_id) REFERENCES melt_quote(id)
                );

                INSERT INTO melt_quote (
                    id, unit, amount, request, fee_reserve, expiry, state,
                    payment_proof, request_lookup_id, created_time, paid_time,
                    payment_method, options, request_lookup_id_kind,
                    estimated_blocks, fee_options, extra_json, selected_fee_index
                ) VALUES (
                    'existing-quote', 'sat', 42, 'invoice', 2, 100, 'PAID',
                    'proof', 'lookup-id', 10, 11, 'bolt11', '{}',
                    'payment_hash', 6, '[]', '{"source":"standalone"}', 0
                );
                INSERT INTO melt_request (quote_id, inputs_amount, inputs_fee)
                VALUES ('existing-quote', 44, 2);
                "#,
            )
            .expect("create pre-federation standalone schema");

        upgraded
            .execute_batch("BEGIN IMMEDIATE")
            .expect("begin failed-state upgrade");
        upgraded
            .execute_batch(include_str!(
                "../../../cdk-sql-common/src/mint/migrations/sqlite/20260707170000_allow_failed_melt_quote_state.sql"
            ))
            .expect("apply failed-state upgrade");
        upgraded
            .execute_batch("COMMIT")
            .expect("commit failed-state upgrade");

        let existing = upgraded
            .query_row(
                r#"
                SELECT state, payment_proof, request_lookup_id, created_time,
                       paid_time, extra_json, selected_fee_index
                FROM melt_quote
                WHERE id = 'existing-quote'
                "#,
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, i64>(6)?,
                    ))
                },
            )
            .expect("read preserved melt quote");
        assert_eq!(
            existing,
            (
                "PAID".to_owned(),
                "proof".to_owned(),
                "lookup-id".to_owned(),
                10,
                11,
                r#"{"source":"standalone"}"#.to_owned(),
                0,
            )
        );
        assert_eq!(
            upgraded
                .query_row(
                    "SELECT COUNT(*) FROM melt_request WHERE quote_id = 'existing-quote'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("preserved melt request count"),
            1
        );
        upgraded
            .execute(
                r#"
                INSERT INTO melt_quote
                    (id, unit, amount, request, fee_reserve, expiry, state)
                VALUES ('failed-quote', 'sat', 1, 'failed', 0, 0, 'FAILED')
                "#,
                [],
            )
            .expect("forward migration admits FAILED");
        assert!(
            upgraded
                .execute(
                    r#"
                    INSERT INTO melt_quote
                        (id, unit, amount, request, fee_reserve, expiry, state)
                    VALUES ('unknown-quote', 'sat', 1, 'unknown', 0, 0, 'UNKNOWN')
                    "#,
                    [],
                )
                .is_err(),
            "backend UNKNOWN must remain transient"
        );

        let mut foreign_key_check = upgraded
            .prepare("PRAGMA foreign_key_check")
            .expect("prepare foreign key check");
        assert!(
            foreign_key_check
                .query([])
                .expect("run foreign key check")
                .next()
                .expect("read foreign key check")
                .is_none(),
            "upgrade must preserve all foreign key relationships"
        );
        drop(foreign_key_check);

        let fresh_path = std::env::temp_dir().join(format!(
            "cdk-test-fresh-failed-state-schema-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let fresh_db = MintSqliteDatabase::new(fresh_path.clone())
            .await
            .expect("fresh database migration");
        drop(fresh_db);
        let fresh = rusqlite::Connection::open(&fresh_path).expect("open fresh database");
        assert_eq!(melt_quote_schema(&upgraded), melt_quote_schema(&fresh));

        drop(fresh);
        let _ = remove_file(fresh_path);
    }

    #[tokio::test]
    async fn dropped_write_transaction_releases_serialized_writer() {
        let path = std::env::temp_dir().to_path_buf().join(format!(
            "cdk-test-sqlite-writer-recovery-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let config: Config = (&path).into();
        let pool = Pool::<SqliteConnectionManager>::new(config);
        let setup = pool.get().await.expect("setup connection");
        query("CREATE TABLE writer_recovery (value INTEGER NOT NULL)")
            .expect("create-table query")
            .execute(&*setup)
            .await
            .expect("create writer recovery table");
        drop(setup);

        let first =
            ConnectionWithTransaction::new(pool.get().await.expect("first writer connection"))
                .await
                .expect("first writer transaction");
        query("INSERT INTO writer_recovery (value) VALUES (1)")
            .expect("first insert query")
            .execute(&first)
            .await
            .expect("first uncommitted insert");

        let waiting_pool = Arc::clone(&pool);
        let second = tokio::spawn(async move {
            let tx = ConnectionWithTransaction::new(
                waiting_pool.get().await.expect("second writer connection"),
            )
            .await
            .expect("second writer transaction");
            query("INSERT INTO writer_recovery (value) VALUES (2)")
                .expect("second insert query")
                .execute(&tx)
                .await
                .expect("second insert");
            tx.commit().await.expect("commit second writer");
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !second.is_finished(),
            "second writer must wait for the active transaction"
        );

        drop(first);
        tokio::time::timeout(Duration::from_secs(2), second)
            .await
            .expect("second writer should recover after rollback")
            .expect("second writer task");

        let verify = pool.get().await.expect("verification connection");
        let rows = query("SELECT value FROM writer_recovery ORDER BY value")
            .expect("verification query")
            .fetch_all(&*verify)
            .await
            .expect("verification rows");
        assert_eq!(rows, vec![vec![cdk_sql_common::value::Value::Integer(2)]]);

        drop(verify);
        drop(pool);
        let _ = remove_file(path);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_config_persistence() {
        cdk_common::database::mint::test::federation_config_persistence(
            provide_db("federation_config_persistence".to_string()).await,
        )
        .await;
    }

    #[tokio::test]
    async fn kvstore_compare_and_swap() {
        cdk_common::database::mint::test::kvstore_compare_and_swap(
            provide_db("test_kvstore_compare_and_swap".to_owned()).await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_journal_persistence() {
        cdk_common::database::mint::test::federation_journal_persistence(
            provide_db("federation_journal_persistence".to_string()).await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_application_state_persistence() {
        cdk_common::database::mint::test::federation_application_state_persistence(
            provide_db("federation_application_state_persistence".to_string()).await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_aleph_bft_finalized_item_persistence() {
        cdk_common::database::mint::test::federation_aleph_bft_finalized_item_persistence(
            provide_db("federation_aleph_bft_finalized_item_persistence".to_string()).await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_checkpoint_persistence() {
        cdk_common::database::mint::test::federation_checkpoint_persistence(
            provide_db("federation_checkpoint_persistence".to_string()).await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_aleph_bft_backup_persistence() {
        cdk_common::database::mint::test::federation_aleph_bft_backup_persistence(
            provide_db("federation_aleph_bft_backup_persistence".to_string()).await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_aleph_bft_active_session_persistence() {
        cdk_common::database::mint::test::federation_aleph_bft_active_session_persistence(
            provide_db("federation_aleph_bft_active_session_persistence".to_string()).await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_aleph_bft_pending_operation_persistence() {
        cdk_common::database::mint::test::federation_aleph_bft_pending_operation_persistence(
            provide_db("federation_aleph_bft_pending_operation_persistence".to_string()).await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_payment_observation_persistence() {
        cdk_common::database::mint::test::federation_payment_observation_persistence(
            provide_db("federation_payment_observation_persistence".to_string()).await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_mint_quote_state_projection() {
        cdk_common::database::mint::test::federation_mint_quote_state_projection(
            provide_db("federation_mint_quote_state_projection".to_string()).await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_materialized_projection_keyed_reads() {
        cdk_common::database::mint::test::federation_materialized_projection_keyed_reads(
            provide_db("federation_materialized_projection_keyed_reads".to_string()).await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_bark_projection_persistence() {
        cdk_common::database::mint::test::federation_bark_projection_persistence(
            provide_db("federation_bark_projection_persistence".to_string()).await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_signature_share_persistence() {
        cdk_common::database::mint::test::federation_signature_share_persistence(
            provide_db("federation_signature_share_persistence".to_string()).await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_dkg_setup_state_persistence() {
        cdk_common::database::mint::test::federation_dkg_setup_state_persistence(
            provide_db("federation_dkg_setup_state_persistence".to_string()).await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_dkg_participant_state_persistence() {
        cdk_common::database::mint::test::federation_dkg_participant_state_persistence(
            provide_db("federation_dkg_participant_state_persistence".to_string()).await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_dkg_ceremony_persistence() {
        cdk_common::database::mint::test::federation_dkg_ceremony_persistence(
            provide_db("federation_dkg_ceremony_persistence".to_string()).await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_dkg_member_keyset_share_persistence() {
        cdk_common::database::mint::test::federation_dkg_member_keyset_share_persistence(
            provide_db("federation_dkg_member_keyset_share_persistence".to_string()).await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_dkg_secret_share_contribution_persistence() {
        cdk_common::database::mint::test::federation_dkg_secret_share_contribution_persistence(
            provide_db("federation_dkg_secret_share_contribution_persistence".to_string()).await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_dkg_transcript_signature_persistence() {
        cdk_common::database::mint::test::federation_dkg_transcript_signature_persistence(
            provide_db("federation_dkg_transcript_signature_persistence".to_string()).await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_dkg_activation_persistence() {
        cdk_common::database::mint::test::federation_dkg_activation_persistence(
            provide_db("federation_dkg_activation_persistence".to_string()).await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_frost_dkg_persistence() {
        cdk_common::database::mint::test::federation_frost_dkg_persistence(
            provide_db("federation_frost_dkg_persistence".to_string()).await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_frost_wallet_config_persistence() {
        cdk_common::database::mint::test::federation_frost_wallet_config_persistence(
            provide_db("federation_frost_wallet_config_persistence".to_string()).await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_dkg_write_failure_rolls_back_and_recovers() {
        use cdk_common::database::mint::{
            Database, FederationDkgDatabase, FederationJournalDatabase,
        };

        let path = std::env::temp_dir().join(format!(
            "cdk-test-dkg-write-failure-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let db = MintSqliteDatabase::new(path.clone())
            .await
            .expect("migrated SQLite DKG database");
        let commitment = cdk_common::database::mint::test::federation_dkg_test_commitment();
        {
            let conn = rusqlite::Connection::open(&path).expect("SQLite fault connection");
            conn.execute_batch(
                r#"
                CREATE TRIGGER fail_federation_dkg_member_sequence
                BEFORE INSERT ON federation_dkg_member_sequences
                BEGIN
                    SELECT RAISE(ABORT, 'simulated SQLite write failure');
                END;
                "#,
            )
            .expect("install SQLite write-failure trigger");
        }

        let mut tx = Database::begin_transaction(&db)
            .await
            .expect("begin failing DKG transaction");
        tx.add_federation_dkg_commitment(&commitment)
            .await
            .expect("first DKG write succeeds before injected failure");
        assert!(tx
            .set_federation_dkg_member_sequence(commitment.member_id, 1)
            .await
            .is_err());
        tx.rollback()
            .await
            .expect("rollback failed DKG transaction deterministically");
        assert!(db
            .get_federation_dkg_commitments()
            .await
            .expect("commitments after rollback")
            .is_empty());
        assert!(db
            .get_federation_dkg_member_sequences()
            .await
            .expect("sequences after rollback")
            .is_empty());

        {
            let conn = rusqlite::Connection::open(&path).expect("SQLite recovery connection");
            conn.execute_batch("DROP TRIGGER fail_federation_dkg_member_sequence;")
                .expect("remove SQLite write-failure trigger");
        }
        let mut tx = Database::begin_transaction(&db)
            .await
            .expect("begin recovered DKG transaction");
        tx.add_federation_dkg_commitment(&commitment)
            .await
            .expect("retry DKG commitment");
        tx.set_federation_dkg_member_sequence(commitment.member_id, 1)
            .await
            .expect("retry member sequence");
        tx.commit().await.expect("commit recovered DKG transaction");
        assert_eq!(
            db.get_federation_dkg_commitments()
                .await
                .expect("recovered commitment"),
            vec![commitment]
        );
        assert_eq!(
            db.get_federation_dkg_member_sequences()
                .await
                .expect("recovered sequence")
                .get(&commitment.member_id),
            Some(&1)
        );

        drop(db);
        let _ = remove_file(&path);
        let _ = remove_file(path.with_extension("sqlite-shm"));
        let _ = remove_file(path.with_extension("sqlite-wal"));
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_journal_concurrent_writes() {
        let path = std::env::temp_dir().to_path_buf().join(format!(
            "cdk-test-federation-concurrent-{}.sqlite",
            uuid::Uuid::new_v4()
        ));

        #[cfg(feature = "sqlcipher")]
        let db = MintSqliteDatabase::new((path.clone(), "password".to_string()))
            .await
            .expect("migrated sqlite database");

        #[cfg(not(feature = "sqlcipher"))]
        let db = MintSqliteDatabase::new(path.clone())
            .await
            .expect("migrated sqlite database");

        cdk_common::database::mint::test::federation_journal_concurrent_writes(db).await;

        let _ = remove_file(path);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_journal_concurrent_metadata_updates() {
        let path = std::env::temp_dir().to_path_buf().join(format!(
            "cdk-test-federation-metadata-{}.sqlite",
            uuid::Uuid::new_v4()
        ));

        #[cfg(feature = "sqlcipher")]
        let db = MintSqliteDatabase::new((path.clone(), "password".to_string()))
            .await
            .expect("migrated sqlite database");

        #[cfg(not(feature = "sqlcipher"))]
        let db = MintSqliteDatabase::new(path.clone())
            .await
            .expect("migrated sqlite database");

        cdk_common::database::mint::test::federation_journal_concurrent_metadata_updates(db).await;

        let _ = remove_file(path);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_aleph_bft_backup_appends_use_independent_rows() {
        use cdk_common::database::mint::{Database, FederationJournalDatabase};

        let path = std::env::temp_dir().join(format!(
            "cdk-test-aleph-backup-chunks-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let db = MintSqliteDatabase::new(path.clone())
            .await
            .expect("migrated SQLite federation database");

        for bytes in [b"first".as_slice(), b"-second", b"-third"] {
            let mut tx = Database::begin_transaction(&db)
                .await
                .expect("begin backup append");
            tx.append_federation_aleph_bft_backup_bytes(7, bytes)
                .await
                .expect("append backup chunk");
            tx.commit().await.expect("commit backup chunk");
        }
        assert_eq!(
            db.get_federation_aleph_bft_backup_bytes(7)
                .await
                .expect("backup bytes"),
            Some(b"first-second-third".to_vec())
        );
        drop(db);

        let conn = rusqlite::Connection::open(&path).expect("SQLite verification connection");
        let stored_rows = conn
            .query_row(
                "SELECT COUNT(*) FROM federation_aleph_bft_backups WHERE session = 7",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("backup chunk row count");
        assert_eq!(
            stored_rows, 3,
            "each flush must insert a small row instead of rewriting the complete backup BLOB"
        );

        drop(conn);
        let _ = remove_file(path);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_aleph_bft_backup_chunk_migration_preserves_existing_bytes() {
        let conn = rusqlite::Connection::open_in_memory().expect("SQLite migration connection");
        conn.execute_batch(
            r#"
            CREATE TABLE federation_aleph_bft_backups (
                session INTEGER PRIMARY KEY NOT NULL,
                backup_bytes BLOB NOT NULL
            );
            INSERT INTO federation_aleph_bft_backups (session, backup_bytes)
            VALUES (7, X'6669727374'), (8, X'7365636F6E64');
            "#,
        )
        .expect("create legacy backup table");

        conn.execute_batch(include_str!(
            "../../../cdk-sql-common/src/mint/migrations/sqlite/20260714000000_chunk_federation_aleph_bft_backups.sql"
        ))
        .expect("migrate legacy backup table");

        let rows = {
            let mut statement = conn
                .prepare(
                    "SELECT session, chunk_index, backup_bytes FROM federation_aleph_bft_backups ORDER BY session",
                )
                .expect("prepare migrated backup query");
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                    ))
                })
                .expect("query migrated backups")
                .collect::<Result<Vec<_>, _>>()
                .expect("read migrated backups")
        };
        assert_eq!(
            rows,
            vec![(7, 0, b"first".to_vec()), (8, 0, b"second".to_vec())]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_migrations_apply_to_empty_sqlite_and_keep_standalone_tables() {
        let path = std::env::temp_dir().to_path_buf().join(format!(
            "cdk-test-federation-empty-migration-{}.sqlite",
            uuid::Uuid::new_v4()
        ));

        #[cfg(feature = "sqlcipher")]
        let db = MintSqliteDatabase::new((path.clone(), "password".to_string()))
            .await
            .expect("fresh federation sqlite database migrates");

        #[cfg(not(feature = "sqlcipher"))]
        let db = MintSqliteDatabase::new(path.clone())
            .await
            .expect("fresh federation sqlite database migrates");

        drop(db);

        #[cfg(feature = "sqlcipher")]
        let reopened = MintSqliteDatabase::new((path.clone(), "password".to_string()))
            .await
            .expect("migrated sqlite database reopens through the normal mint path");

        #[cfg(not(feature = "sqlcipher"))]
        let reopened = MintSqliteDatabase::new(path.clone())
            .await
            .expect("migrated sqlite database reopens through the normal mint path");

        drop(reopened);

        #[cfg(feature = "sqlcipher")]
        let config: Config = (path.clone(), "password".to_string()).into();
        #[cfg(not(feature = "sqlcipher"))]
        let config: Config = (&path).into();

        let pool = Pool::<SqliteConnectionManager>::new(config);
        let conn = pool.get().await.expect("valid connection");

        let table_rows = query(
            r#"
            SELECT name
            FROM sqlite_master
            WHERE type = 'table'
            "#,
        )
        .expect("table query")
        .fetch_all(&*conn)
        .await
        .expect("sqlite table rows");
        let table_names = table_rows
            .into_iter()
            .map(|row| text_column(&row, 0))
            .collect::<BTreeSet<_>>();

        for table in [
            "proof",
            "keyset",
            "mint_quote",
            "melt_quote",
            "blind_signature",
            "saga_state",
            "completed_operations",
            "federation_journal_entries",
            "federation_dkg_member_sequences",
            "federation_checkpoints",
            "federation_checkpoint_signatures",
            "federation_checkpoint_quorums",
            "federation_checkpoint_state",
            "federation_materialized_state_keys",
            "federation_aleph_bft_finalized_items",
            "federation_aleph_bft_backups",
            "federation_aleph_bft_pending_operations",
            "federation_aleph_bft_session_state",
            "federation_payment_observations",
            "federation_config",
            "federation_signature_shares",
            "federation_dkg_setup_messages",
            "federation_dkg_participant_states",
            "federation_dkg_member_keyset_shares",
            "federation_dkg_secret_share_contributions",
            "federation_dkg_transcript_signatures",
            "federation_dkg_activations",
            "federation_dkg_ceremonies",
            "federation_frost_dkg_records",
            "federation_frost_wallet_config",
            "federation_bark_actions",
            "federation_bark_vtxos",
            "federation_bark_derivation_allocations",
            "federation_bark_reservations",
            "federation_bark_transitions",
            "federation_bark_observations",
        ] {
            assert!(
                table_names.contains(table),
                "missing migrated table {table}"
            );
        }
        assert!(
            !table_names.contains("federation_aleph_bft_network_retries"),
            "AlephBFT gossip retries must remain runtime-only"
        );

        let federation_migrations = query(
            r#"
            SELECT name
            FROM migrations
            WHERE name LIKE '%federation%'
            ORDER BY name
            "#,
        )
        .expect("federation migration query")
        .fetch_all(&*conn)
        .await
        .expect("federation migration rows")
        .into_iter()
        .map(|row| text_column(&row, 0))
        .collect::<Vec<_>>();
        assert_eq!(
            federation_migrations,
            vec![
                "20260712000000_create_federation_schema.sql".to_owned(),
                "20260713000000_allow_federation_payment_observation_revisions.sql".to_owned(),
                "20260714000000_chunk_federation_aleph_bft_backups.sql".to_owned(),
                "20260719000000_create_federation_bark_projection.sql".to_owned(),
            ]
        );

        let journal_columns = query("PRAGMA table_info(federation_journal_entries)")
            .expect("journal column query")
            .fetch_all(&*conn)
            .await
            .expect("journal column rows")
            .into_iter()
            .map(|row| {
                let name = text_column(&row, 1);
                let required = matches!(row.get(3), Some(Value::Integer(1)));
                (name, required)
            })
            .collect::<BTreeMap<_, _>>();
        assert_eq!(journal_columns.get("conflict_keys_json"), Some(&true));
        assert!(!journal_columns.contains_key("operation_outcome_hash"));
        assert!(!journal_columns.contains_key("materialized_state_digest"));
        for column in [
            "consensus_evidence_json",
            "rejection_json",
            "application_digest",
        ] {
            assert_eq!(journal_columns.get(column), Some(&false));
        }

        let rejection_on_applied = query(
            r#"
            INSERT INTO federation_journal_entries
            (operation_index, operation_id, envelope_json, conflict_keys_json, previous_order_digest, order_digest, rejection_json, status)
            VALUES (0, 'invalid-rejection', '{}', '[]', 'previous', 'current', '{}', 'applied')
            "#,
        )
        .expect("invalid application-state insert")
        .execute(&*conn)
        .await;
        assert!(rejection_on_applied.is_err());

        let unknown_status = query(
            r#"
            INSERT INTO federation_journal_entries
            (operation_index, operation_id, envelope_json, conflict_keys_json, previous_order_digest, order_digest, status)
            VALUES (1, 'invalid-status', '{}', '[]', 'previous', 'current', 'unknown')
            "#,
        )
        .expect("unknown application-state insert")
        .execute(&*conn)
        .await;
        assert!(unknown_status.is_err());

        query(
            r#"
            INSERT INTO melt_quote
                (id, unit, amount, request, fee_reserve, expiry, state)
            VALUES
                ('state-unpaid', 'sat', 1, 'request-unpaid', 0, 0, 'UNPAID'),
                ('state-pending', 'sat', 1, 'request-pending', 0, 0, 'PENDING'),
                ('state-paid', 'sat', 1, 'request-paid', 0, 0, 'PAID'),
                ('state-failed', 'sat', 1, 'request-failed', 0, 0, 'FAILED')
            "#,
        )
        .expect("durable melt quote state insert")
        .execute(&*conn)
        .await
        .expect("migrated melt quote schema accepts every durable state");

        let unknown_melt_quote_state = query(
            r#"
            INSERT INTO melt_quote
                (id, unit, amount, request, fee_reserve, expiry, state)
            VALUES
                ('state-unknown', 'sat', 1, 'request-unknown', 0, 0, 'UNKNOWN')
            "#,
        )
        .expect("transient melt quote state insert")
        .execute(&*conn)
        .await;
        assert!(
            unknown_melt_quote_state.is_err(),
            "migrated melt quote schema must reject transient backend UNKNOWN"
        );

        let saga_columns = query("PRAGMA table_info(saga_state)")
            .expect("saga column query")
            .fetch_all(&*conn)
            .await
            .expect("saga column rows")
            .into_iter()
            .map(|row| text_column(&row, 1))
            .collect::<BTreeSet<_>>();
        assert!(saga_columns.contains("federation_operation_id"));

        drop(conn);
        drop(pool);
        let _ = remove_file(path);
    }

    #[cfg(all(feature = "federation", not(feature = "sqlcipher")))]
    #[tokio::test]
    async fn pre_bark_sqlite_database_upgrades_with_only_the_forward_migration() {
        let path = std::env::temp_dir().to_path_buf().join(format!(
            "cdk-test-federation-pre-bark-migration-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let db = MintSqliteDatabase::new(path.clone())
            .await
            .expect("create current SQLite database");
        drop(db);

        let conn = rusqlite::Connection::open(&path).expect("open migration fixture");
        conn.execute_batch(
            r#"
            DROP TABLE federation_bark_observations;
            DROP TABLE federation_bark_transitions;
            DROP TABLE federation_bark_reservations;
            DROP TABLE federation_bark_derivation_allocations;
            DROP TABLE federation_bark_vtxos;
            DROP TABLE federation_bark_actions;
            DELETE FROM migrations
            WHERE name = '20260719000000_create_federation_bark_projection.sql';
            "#,
        )
        .expect("rewind fixture to the pre-Bark schema");
        drop(conn);

        let upgraded = MintSqliteDatabase::new(path.clone())
            .await
            .expect("apply only the forward Bark migration");
        drop(upgraded);
        let conn = rusqlite::Connection::open(&path).expect("open upgraded fixture");
        for table in [
            "federation_bark_actions",
            "federation_bark_vtxos",
            "federation_bark_derivation_allocations",
            "federation_bark_reservations",
            "federation_bark_transitions",
            "federation_bark_observations",
        ] {
            let count = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |row| row.get::<_, i64>(0),
                )
                .expect("query Bark projection table");
            assert_eq!(count, 1, "missing upgraded Bark table {table}");
        }
        let proof_table = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'proof'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("query historical standalone table");
        assert_eq!(proof_table, 1);
        drop(conn);
        let _ = remove_file(path);
    }

    #[cfg(all(feature = "federation", not(feature = "sqlcipher")))]
    #[tokio::test]
    async fn bark_projection_migration_failure_rolls_back_atomically() {
        let path = std::env::temp_dir().to_path_buf().join(format!(
            "cdk-test-federation-bark-migration-rollback-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let db = MintSqliteDatabase::new(path.clone())
            .await
            .expect("create current SQLite database");
        drop(db);

        let conn = rusqlite::Connection::open(&path).expect("open migration fixture");
        conn.execute_batch(
            r#"
            DROP TABLE federation_bark_observations;
            DROP TABLE federation_bark_transitions;
            DROP TABLE federation_bark_reservations;
            DROP TABLE federation_bark_derivation_allocations;
            DROP TABLE federation_bark_vtxos;
            DROP TABLE federation_bark_actions;
            DELETE FROM migrations
            WHERE name = '20260719000000_create_federation_bark_projection.sql';
            CREATE TABLE federation_bark_transitions (conflict TEXT NOT NULL);
            "#,
        )
        .expect("prepare a late migration conflict");
        drop(conn);

        assert!(
            MintSqliteDatabase::new(path.clone()).await.is_err(),
            "the deliberately conflicting migration must fail"
        );
        let conn = rusqlite::Connection::open(&path).expect("open rolled-back fixture");
        let actions = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'federation_bark_actions'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("query rolled-back action table");
        assert_eq!(actions, 0, "earlier statements must roll back on failure");
        let migration_marker = conn
            .query_row(
                "SELECT COUNT(*) FROM migrations WHERE name = '20260719000000_create_federation_bark_projection.sql'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("query Bark migration marker");
        assert_eq!(migration_marker, 0);
        drop(conn);
        let _ = remove_file(path);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_journal_hot_path_indexes_are_used() {
        let path = std::env::temp_dir().to_path_buf().join(format!(
            "cdk-test-federation-indexes-{}.sqlite",
            uuid::Uuid::new_v4()
        ));

        #[cfg(feature = "sqlcipher")]
        let db = MintSqliteDatabase::new((path.clone(), "password".to_string()))
            .await
            .expect("migrated sqlite database");

        #[cfg(not(feature = "sqlcipher"))]
        let db = MintSqliteDatabase::new(path.clone())
            .await
            .expect("migrated sqlite database");

        drop(db);

        #[cfg(feature = "sqlcipher")]
        let config: Config = (path.clone(), "password".to_string()).into();
        #[cfg(not(feature = "sqlcipher"))]
        let config: Config = (&path).into();

        let pool = Pool::<SqliteConnectionManager>::new(config);
        let conn = pool.get().await.expect("valid connection");

        let index_rows = query(
            r#"
            SELECT name
            FROM sqlite_master
            WHERE type = 'index'
              AND tbl_name IN (
                  'federation_journal_entries',
                  'federation_checkpoints',
                  'federation_checkpoint_signatures',
                  'federation_materialized_state_keys',
                  'federation_aleph_bft_finalized_items',
                  'federation_aleph_bft_pending_operations',
                  'federation_payment_observations',
                  'federation_signature_shares',
                  'federation_dkg_setup_messages',
                  'federation_dkg_participant_states',
                  'federation_dkg_member_keyset_shares',
                  'federation_dkg_secret_share_contributions',
                  'federation_dkg_transcript_signatures',
                  'federation_dkg_activations',
                  'federation_bark_actions',
                  'federation_bark_vtxos',
                  'federation_bark_derivation_allocations',
                  'federation_bark_reservations',
                  'federation_bark_transitions',
                  'federation_bark_observations',
                  'blind_signature'
              )
            "#,
        )
        .expect("index query")
        .fetch_all(&*conn)
        .await
        .expect("federation index rows");
        let index_names = index_rows
            .into_iter()
            .map(|row| text_column(&row, 0))
            .collect::<BTreeSet<_>>();
        assert!(
            index_names.contains("idx_federation_journal_operation_id"),
            "missing federation operation-id index"
        );
        assert!(
            index_names.contains("idx_federation_checkpoints_kind_sequence"),
            "missing federation checkpoint kind/sequence index"
        );
        assert!(
            index_names.contains("idx_federation_checkpoints_finalized_cursor"),
            "missing federation checkpoint finalized-cursor index"
        );
        assert!(
            index_names.contains("idx_federation_checkpoint_signatures_checkpoint"),
            "missing federation checkpoint-signature lookup index"
        );
        assert!(
            index_names.contains("idx_federation_materialized_state_keys_operation"),
            "missing federation materialized-state operation index"
        );
        assert!(
            index_names.contains("idx_federation_aleph_bft_finalized_items_session_unit"),
            "missing federation AlephBFT finalized-item session/unit index"
        );
        assert!(
            index_names.contains("idx_federation_aleph_bft_finalized_items_operation_id"),
            "missing federation AlephBFT finalized-item operation-id index"
        );
        assert!(
            index_names.contains("idx_federation_aleph_bft_pending_operations_session"),
            "missing federation AlephBFT pending-operation session index"
        );
        assert!(
            index_names.contains("idx_federation_payment_observations_member_state"),
            "missing federation payment-observation member-state index"
        );
        assert!(
            index_names.contains("idx_federation_payment_observations_quote"),
            "missing federation payment-observation quote index"
        );
        for index in [
            "idx_federation_bark_actions_phase",
            "idx_federation_bark_vtxos_state",
            "idx_federation_bark_vtxos_reservation",
            "idx_federation_bark_allocations_action",
            "idx_federation_bark_reservations_action",
            "idx_federation_bark_reservations_operation",
            "idx_federation_bark_transitions_action",
            "idx_federation_bark_observations_action",
        ] {
            assert!(
                index_names.contains(index),
                "missing migrated index {index}"
            );
        }
        assert!(
            index_names.contains("idx_federation_signature_shares_operation_output"),
            "missing federation signature-share operation/output index"
        );
        assert!(
            index_names.contains("idx_federation_signature_shares_blinded_message"),
            "missing federation signature-share blinded-message index"
        );
        assert!(
            index_names.contains("idx_federation_dkg_setup_member"),
            "missing federation DKG setup member index"
        );
        assert!(
            index_names.contains("idx_federation_dkg_participant_states_member"),
            "missing federation DKG participant-state member index"
        );
        assert!(
            index_names.contains("idx_federation_dkg_member_keyset_shares_member"),
            "missing federation DKG member keyset-share member index"
        );
        assert!(
            index_names.contains("idx_federation_dkg_secret_share_contributions_receiver"),
            "missing federation DKG secret-share receiver index"
        );
        assert!(
            index_names.contains("idx_federation_dkg_transcript_signatures_member"),
            "missing federation DKG transcript-signature member index"
        );
        assert!(
            index_names.contains("idx_federation_dkg_activations_member"),
            "missing federation DKG activation member index"
        );
        assert!(
            index_names.contains("blind_signature_quote_id_index"),
            "missing blind-signature quote-id index"
        );

        let journal_range_plan = explain_details(
            query(
                r#"
                EXPLAIN QUERY PLAN
                SELECT operation_index, operation_id
                FROM federation_journal_entries
                WHERE operation_index >= 1
                ORDER BY operation_index ASC
                "#,
            )
            .expect("journal range plan query")
            .fetch_all(&*conn)
            .await
            .expect("journal range query plan"),
        );
        assert!(
            journal_range_plan
                .iter()
                .any(|detail| detail.contains("USING INTEGER PRIMARY KEY")),
            "journal range catch-up should use the operation_index primary key: {journal_range_plan:?}"
        );

        let operation_id_plan = explain_details(
            query(
                r#"
                EXPLAIN QUERY PLAN
                UPDATE federation_journal_entries
                SET status = 'applied'
                WHERE operation_id = '0000000000000000000000000000000000000000000000000000000000000000'
                "#,
            )
            .expect("operation-id plan query")
            .fetch_all(&*conn)
            .await
            .expect("operation-id query plan"),
        );
        assert_uses_indexed_search(&operation_id_plan, "federation_journal_entries");

        let operation_lookup_plan = explain_details(
            query(
                r#"
                EXPLAIN QUERY PLAN
                SELECT operation_index, operation_id, consensus_evidence_json, status
                FROM federation_journal_entries
                WHERE operation_id = '0000000000000000000000000000000000000000000000000000000000000000'
                "#,
            )
            .expect("operation lookup plan query")
            .fetch_all(&*conn)
            .await
            .expect("operation lookup query plan"),
        );
        assert_uses_indexed_search(&operation_lookup_plan, "federation_journal_entries");

        let payment_observation_plan = explain_details(
            query(
                r#"
                EXPLAIN QUERY PLAN
                SELECT operation_id, operation_index, observation_json
                FROM federation_payment_observations
                WHERE context = 'mint_quote'
                  AND quote = 'quote-1'
                ORDER BY operation_index ASC, observer_member_id ASC
                "#,
            )
            .expect("payment-observation plan query")
            .fetch_all(&*conn)
            .await
            .expect("payment-observation query plan"),
        );
        assert_uses_indexed_search(&payment_observation_plan, "federation_payment_observations");

        let signature_share_output_plan = explain_details(
            query(
                r#"
                EXPLAIN QUERY PLAN
                SELECT share_json
                FROM federation_signature_shares
                WHERE operation_id = '0000000000000000000000000000000000000000000000000000000000000000'
                  AND output_index = 0
                ORDER BY member_id ASC
                "#,
            )
            .expect("signature-share output plan query")
            .fetch_all(&*conn)
            .await
            .expect("signature-share output query plan"),
        );
        assert_uses_indexed_search(&signature_share_output_plan, "federation_signature_shares");

        let signature_share_blinded_message_plan = explain_details(
            query(
                r#"
                EXPLAIN QUERY PLAN
                SELECT share_json
                FROM federation_signature_shares
                WHERE blinded_message = x'000102'
                ORDER BY operation_id ASC, output_index ASC, member_id ASC
                "#,
            )
            .expect("signature-share blinded-message plan query")
            .fetch_all(&*conn)
            .await
            .expect("signature-share blinded-message query plan"),
        );
        assert_uses_indexed_search(
            &signature_share_blinded_message_plan,
            "federation_signature_shares",
        );

        let proof_y_plan = explain_details(
            query(
                r#"
                EXPLAIN QUERY PLAN
                SELECT state
                FROM proof
                WHERE y = x'000102'
                "#,
            )
            .expect("proof-y plan query")
            .fetch_all(&*conn)
            .await
            .expect("proof-y query plan"),
        );
        assert_uses_indexed_search(&proof_y_plan, "proof");

        let blind_signature_blinded_message_plan = explain_details(
            query(
                r#"
                EXPLAIN QUERY PLAN
                SELECT c
                FROM blind_signature
                WHERE blinded_message = x'000102'
                "#,
            )
            .expect("blind-signature blinded-message plan query")
            .fetch_all(&*conn)
            .await
            .expect("blind-signature blinded-message query plan"),
        );
        assert_uses_indexed_search(&blind_signature_blinded_message_plan, "blind_signature");

        let blind_signature_quote_id_plan = explain_details(
            query(
                r#"
                EXPLAIN QUERY PLAN
                SELECT blinded_message
                FROM blind_signature
                WHERE quote_id = 'quote-1'
                  AND c IS NOT NULL
                ORDER BY order_index ASC
                "#,
            )
            .expect("blind-signature quote-id plan query")
            .fetch_all(&*conn)
            .await
            .expect("blind-signature quote-id query plan"),
        );
        assert_uses_indexed_search(&blind_signature_quote_id_plan, "blind_signature");

        drop(conn);
        drop(pool);
        let _ = remove_file(path);
    }

    #[cfg(feature = "federation")]
    fn explain_details(rows: Vec<Vec<Value>>) -> Vec<String> {
        rows.into_iter().map(|row| text_column(&row, 3)).collect()
    }

    #[cfg(feature = "federation")]
    fn text_column(row: &[Value], index: usize) -> String {
        match row.get(index).expect("query column") {
            Value::Text(value) => value.clone(),
            _ => panic!("expected text column"),
        }
    }

    #[cfg(feature = "federation")]
    fn assert_uses_indexed_search(plan: &[String], table: &str) {
        assert!(
            plan.iter().any(|detail| detail.contains(table)
                && detail.contains("SEARCH")
                && detail.contains("USING")),
            "{table} query should use an indexed search: {plan:?}"
        );
        assert!(
            !plan
                .iter()
                .any(|detail| detail.contains(&format!("SCAN {table}"))),
            "{table} query should not fall back to a table scan: {plan:?}"
        );
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

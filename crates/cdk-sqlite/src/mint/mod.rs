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
    use std::collections::BTreeSet;
    use std::fs::remove_file;
    use std::time::Duration;

    use cdk_common::mint_db_test;
    use cdk_sql_common::pool::Pool;
    use cdk_sql_common::stmt::query;
    use cdk_sql_common::value::Value;

    use super::*;
    use crate::common::Config;

    async fn provide_db(_test_name: String) -> MintSqliteDatabase {
        memory::empty().await.unwrap()
    }

    mint_db_test!(provide_db);

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
    async fn federation_payment_observation_persistence() {
        cdk_common::database::mint::test::federation_payment_observation_persistence(
            provide_db("federation_payment_observation_persistence".to_string()).await,
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
    async fn federation_dkg_member_keyset_share_persistence() {
        cdk_common::database::mint::test::federation_dkg_member_keyset_share_persistence(
            provide_db("federation_dkg_member_keyset_share_persistence".to_string()).await,
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
                  'federation_session_outcomes',
                  'federation_payment_observations',
                  'federation_dkg_setup_messages',
                  'federation_dkg_member_keyset_shares',
                  'federation_dkg_transcript_signatures'
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
            index_names.contains("idx_federation_session_outcomes_from_index"),
            "missing federation session-outcome range index"
        );
        assert!(
            index_names.contains("idx_federation_payment_observations_member_state"),
            "missing federation payment-observation member-state index"
        );
        assert!(
            index_names.contains("idx_federation_payment_observations_quote"),
            "missing federation payment-observation quote index"
        );
        assert!(
            index_names.contains("idx_federation_dkg_setup_member"),
            "missing federation DKG setup member index"
        );
        assert!(
            index_names.contains("idx_federation_dkg_member_keyset_shares_member"),
            "missing federation DKG member keyset-share member index"
        );
        assert!(
            index_names.contains("idx_federation_dkg_transcript_signatures_member"),
            "missing federation DKG transcript-signature member index"
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

        let session_outcome_plan = explain_details(
            query(
                r#"
                EXPLAIN QUERY PLAN
                SELECT outcome_json
                FROM federation_session_outcomes
                WHERE from_index >= 1
                ORDER BY from_index ASC, signer_member_id ASC
                "#,
            )
            .expect("session-outcome plan query")
            .fetch_all(&*conn)
            .await
            .expect("session-outcome query plan"),
        );
        assert_uses_indexed_search(&session_outcome_plan, "federation_session_outcomes");

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

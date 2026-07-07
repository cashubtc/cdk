//! SQL database implementation of the Mint
//!
//! This is a generic SQL implementation for the mint storage layer. Any database can be plugged in
//! as long as standard ANSI SQL is used, as Postgres and SQLite would understand it.
//!
//! This implementation also has a rudimentary but standard migration and versioning system.
//!
//! The trait expects an asynchronous interaction, but it also provides tools to spawn blocking
//! clients in a pool and expose them to an asynchronous environment, making them compatible with
//! Mint.
use std::fmt::Debug;
use std::sync::Arc;

use async_trait::async_trait;
use cdk_common::database::{self, DbTransactionFinalizer, Error, MintDatabase};

use crate::common::migrate;
use crate::database::{ConnectionWithTransaction, DatabaseExecutor};
use crate::pool::{DatabasePool, Pool, PooledResource};

mod auth;
mod completed_operations;
#[cfg(feature = "federation")]
mod federation;
mod keys;
mod keyvalue;
mod proofs;
mod quotes;
mod saga;
mod signatures;

#[rustfmt::skip]
mod migrations {
    include!(concat!(env!("OUT_DIR"), "/migrations_mint.rs"));
}

pub use auth::SQLMintAuthDatabase;
#[cfg(feature = "prometheus")]
use cdk_prometheus::MintMetricGuard;
use migrations::MIGRATIONS;

/// Mint SQL Database
#[derive(Debug, Clone)]
pub struct SQLMintDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    pub(crate) pool: Arc<Pool<RM>>,
}

/// SQL Transaction Writer
#[allow(missing_debug_implementations)]
pub struct SQLTransaction<RM>
where
    RM: DatabasePool + 'static,
{
    pub(crate) inner: ConnectionWithTransaction<RM::Connection, PooledResource<RM>>,
}

impl<RM> SQLMintDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    /// Creates a new instance
    pub async fn new<X>(db: X) -> Result<Self, Error>
    where
        X: Into<RM::Config>,
    {
        let pool = Pool::new(db.into());

        Self::migrate(pool.get().await.map_err(|e| Error::Database(Box::new(e)))?).await?;

        Ok(Self { pool })
    }

    /// Migrate
    async fn migrate(conn: PooledResource<RM>) -> Result<(), Error> {
        let tx = ConnectionWithTransaction::new(conn).await?;
        migrate(&tx, RM::Connection::name(), MIGRATIONS).await?;
        tx.commit().await?;
        Ok(())
    }
}

#[async_trait]
impl<RM> database::MintTransaction<Error> for SQLTransaction<RM> where RM: DatabasePool + 'static {}

#[async_trait]
impl<RM> DbTransactionFinalizer for SQLTransaction<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn commit(self: Box<Self>) -> Result<(), Error> {
        #[cfg(feature = "prometheus")]
        let metrics = MintMetricGuard::new("transaction_commit");

        let result = self.inner.commit().await;

        #[cfg(feature = "prometheus")]
        {
            metrics.record(result.is_ok());
        }

        Ok(result?)
    }

    async fn rollback(self: Box<Self>) -> Result<(), Error> {
        #[cfg(feature = "prometheus")]
        let metrics = MintMetricGuard::new("transaction_rollback");

        let result = self.inner.rollback().await;

        #[cfg(feature = "prometheus")]
        {
            metrics.record(result.is_ok());
        }
        Ok(result?)
    }
}

#[async_trait]
impl<RM> MintDatabase<Error> for SQLMintDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    async fn begin_transaction(
        &self,
    ) -> Result<Box<dyn database::MintTransaction<Error> + Send + Sync>, Error> {
        let tx = SQLTransaction {
            inner: ConnectionWithTransaction::new(
                self.pool
                    .get()
                    .await
                    .map_err(|e| Error::Database(Box::new(e)))?,
            )
            .await?,
        };

        Ok(Box::new(tx))
    }
}

#[cfg(all(test, feature = "prometheus"))]
mod tests {
    use std::fmt;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::Duration;

    use cdk_common::database::{DbTransactionFinalizer, Error as DatabaseError};
    use cdk_prometheus::METRICS;

    use super::SQLTransaction;
    use crate::database::{
        ConnectionWithTransaction, DatabaseConnector, DatabaseExecutor, DatabaseTransaction,
    };
    use crate::pool::{DatabaseConfig, DatabasePool, Error as PoolError, Pool};
    use crate::stmt::{Column, Statement};

    #[derive(Debug, Clone)]
    struct TestConfig {
        fail_commit: bool,
        fail_rollback: bool,
    }

    impl DatabaseConfig for TestConfig {
        fn max_size(&self) -> usize {
            1
        }

        fn default_timeout(&self) -> Duration {
            Duration::from_millis(10)
        }
    }

    #[derive(Debug)]
    struct TestResourceError;

    impl fmt::Display for TestResourceError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("test resource error")
        }
    }

    impl std::error::Error for TestResourceError {}

    #[derive(Debug)]
    struct TestConnection {
        fail_commit: bool,
        fail_rollback: bool,
    }

    #[async_trait::async_trait]
    impl DatabaseExecutor for TestConnection {
        fn name() -> &'static str {
            "test"
        }

        async fn execute(&self, _statement: Statement) -> Result<usize, DatabaseError> {
            Ok(0)
        }

        async fn fetch_one(
            &self,
            _statement: Statement,
        ) -> Result<Option<Vec<Column>>, DatabaseError> {
            Ok(None)
        }

        async fn fetch_all(
            &self,
            _statement: Statement,
        ) -> Result<Vec<Vec<Column>>, DatabaseError> {
            Ok(Vec::new())
        }

        async fn pluck(&self, _statement: Statement) -> Result<Option<Column>, DatabaseError> {
            Ok(None)
        }

        async fn batch(&self, _statement: Statement) -> Result<(), DatabaseError> {
            Ok(())
        }
    }

    #[derive(Debug)]
    struct TestTransaction;

    #[async_trait::async_trait]
    impl DatabaseTransaction<TestConnection> for TestTransaction {
        async fn commit(conn: &mut TestConnection) -> Result<(), DatabaseError> {
            if conn.fail_commit {
                Err(DatabaseError::Internal("commit failed".to_owned()))
            } else {
                Ok(())
            }
        }

        async fn begin(_conn: &mut TestConnection) -> Result<(), DatabaseError> {
            Ok(())
        }

        async fn rollback(conn: &mut TestConnection) -> Result<(), DatabaseError> {
            if conn.fail_rollback {
                Err(DatabaseError::Internal("rollback failed".to_owned()))
            } else {
                Ok(())
            }
        }
    }

    impl DatabaseConnector for TestConnection {
        type Transaction = TestTransaction;
    }

    #[derive(Debug)]
    struct TestPool;

    impl DatabasePool for TestPool {
        type Connection = TestConnection;
        type Config = TestConfig;
        type Error = TestResourceError;

        fn new_resource(
            config: &Self::Config,
            _stale: Arc<AtomicBool>,
            _timeout: Duration,
        ) -> Result<Self::Connection, PoolError<Self::Error>> {
            Ok(TestConnection {
                fail_commit: config.fail_commit,
                fail_rollback: config.fail_rollback,
            })
        }
    }

    async fn new_transaction(fail_commit: bool, fail_rollback: bool) -> SQLTransaction<TestPool> {
        let pool = Pool::<TestPool>::new(TestConfig {
            fail_commit,
            fail_rollback,
        });
        let conn = pool
            .get()
            .await
            .expect("test resource should be checked out");
        let inner = ConnectionWithTransaction::new(conn)
            .await
            .expect("test transaction should begin");

        SQLTransaction { inner }
    }

    fn labels_match(
        metric: &cdk_prometheus::prometheus::proto::Metric,
        labels: &[(&str, &str)],
    ) -> bool {
        labels.iter().all(|(name, value)| {
            metric
                .get_label()
                .iter()
                .any(|label| label.get_name() == *name && label.get_value() == *value)
        })
    }

    fn counter_value(name: &str, labels: &[(&str, &str)]) -> f64 {
        for family in METRICS.registry().gather() {
            if family.get_name() != name {
                continue;
            }

            for metric in family.get_metric() {
                if labels_match(metric, labels) {
                    return metric.get_counter().get_value();
                }
            }
        }

        0.0
    }

    fn gauge_value(name: &str, labels: &[(&str, &str)]) -> f64 {
        for family in METRICS.registry().gather() {
            if family.get_name() != name {
                continue;
            }

            for metric in family.get_metric() {
                if labels_match(metric, labels) {
                    return metric.get_gauge().get_value();
                }
            }
        }

        0.0
    }

    fn histogram_count(name: &str, labels: &[(&str, &str)]) -> f64 {
        for family in METRICS.registry().gather() {
            if family.get_name() != name {
                continue;
            }

            for metric in family.get_metric() {
                if labels_match(metric, labels) {
                    return metric.get_histogram().get_sample_count() as f64;
                }
            }
        }

        0.0
    }

    #[tokio::test(flavor = "current_thread")]
    async fn transaction_commit_records_success_duration_and_balances_in_flight() {
        let _lock = crate::metrics_test_lock::lock().await;
        let operation = "transaction_commit";
        let labels = [("operation", operation), ("status", "success")];
        let in_flight_labels = [("operation", operation)];

        let success_before = counter_value("cdk_mint_operations_total", &labels);
        let duration_count_before = histogram_count("cdk_mint_operation_duration_seconds", &labels);
        let in_flight_before = gauge_value("cdk_mint_in_flight_requests", &in_flight_labels);

        let tx = new_transaction(false, false).await;
        Box::new(tx)
            .commit()
            .await
            .expect("transaction commit should succeed");

        assert_eq!(
            counter_value("cdk_mint_operations_total", &labels),
            success_before + 1.0
        );
        assert_eq!(
            histogram_count("cdk_mint_operation_duration_seconds", &labels),
            duration_count_before + 1.0
        );
        assert_eq!(
            gauge_value("cdk_mint_in_flight_requests", &in_flight_labels),
            in_flight_before
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn transaction_commit_records_error_duration_and_balances_in_flight() {
        let _lock = crate::metrics_test_lock::lock().await;
        let operation = "transaction_commit";
        let labels = [("operation", operation), ("status", "error")];
        let in_flight_labels = [("operation", operation)];

        let error_before = counter_value("cdk_mint_operations_total", &labels);
        let duration_count_before = histogram_count("cdk_mint_operation_duration_seconds", &labels);
        let in_flight_before = gauge_value("cdk_mint_in_flight_requests", &in_flight_labels);

        let tx = new_transaction(true, false).await;
        Box::new(tx)
            .commit()
            .await
            .expect_err("transaction commit should fail");

        assert_eq!(
            counter_value("cdk_mint_operations_total", &labels),
            error_before + 1.0
        );
        assert_eq!(
            histogram_count("cdk_mint_operation_duration_seconds", &labels),
            duration_count_before + 1.0
        );
        assert_eq!(
            gauge_value("cdk_mint_in_flight_requests", &in_flight_labels),
            in_flight_before
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn transaction_rollback_records_success_duration_and_balances_in_flight() {
        let _lock = crate::metrics_test_lock::lock().await;
        let operation = "transaction_rollback";
        let labels = [("operation", operation), ("status", "success")];
        let in_flight_labels = [("operation", operation)];

        let success_before = counter_value("cdk_mint_operations_total", &labels);
        let duration_count_before = histogram_count("cdk_mint_operation_duration_seconds", &labels);
        let in_flight_before = gauge_value("cdk_mint_in_flight_requests", &in_flight_labels);

        let tx = new_transaction(false, false).await;
        Box::new(tx)
            .rollback()
            .await
            .expect("transaction rollback should succeed");

        assert_eq!(
            counter_value("cdk_mint_operations_total", &labels),
            success_before + 1.0
        );
        assert_eq!(
            histogram_count("cdk_mint_operation_duration_seconds", &labels),
            duration_count_before + 1.0
        );
        assert_eq!(
            gauge_value("cdk_mint_in_flight_requests", &in_flight_labels),
            in_flight_before
        );
    }
}

#[cfg(all(test, feature = "federation"))]
mod federation_migration_tests {
    fn assert_contains_all(sql: &str, expected: &[&str]) {
        for needle in expected {
            assert!(
                sql.contains(needle),
                "migration is missing `{needle}` in:\n{sql}"
            );
        }
    }

    fn compact_sql(sql: &str) -> String {
        sql.chars()
            .filter(|character| !character.is_whitespace())
            .collect()
    }

    fn assert_melt_quote_state_constraint_keeps_unknown_transient(name: &str, sql: &str) {
        let compact = compact_sql(sql);
        let mut remainder = compact.as_str();

        while let Some(index) = remainder.find("melt_quote") {
            remainder = &remainder[index..];
            let statement_end = remainder.find(';').unwrap_or(remainder.len());
            let statement = &remainder[..statement_end];

            if statement.contains("stateTEXTCHECK") || statement.contains("ADDstateTEXTCHECK") {
                assert!(
                    !statement.contains("'UNKNOWN'"),
                    "melt quote schema must not persist backend Unknown as a durable state in {name}:\n{sql}"
                );
            }

            if statement_end == remainder.len() {
                break;
            }

            remainder = &remainder[statement_end + 1..];
        }
    }

    #[test]
    fn federation_mint_migrations_cover_sqlite_and_postgres() {
        let sqlite_saga =
            include_str!("migrations/sqlite/20260703120000_add_saga_federation_operation_id.sql");
        let postgres_saga =
            include_str!("migrations/postgres/20260703120000_add_saga_federation_operation_id.sql");
        let sqlite_journal =
            include_str!("migrations/sqlite/20260703130000_add_federation_journal.sql");
        let postgres_journal =
            include_str!("migrations/postgres/20260703130000_add_federation_journal.sql");
        let sqlite_journal_consensus_evidence = include_str!(
            "migrations/sqlite/20260703164000_add_federation_journal_consensus_evidence.sql"
        );
        let postgres_journal_consensus_evidence = include_str!(
            "migrations/postgres/20260703164000_add_federation_journal_consensus_evidence.sql"
        );
        let sqlite_aleph_bft_finalized_items = include_str!(
            "migrations/sqlite/20260703165000_add_federation_aleph_bft_finalized_items.sql"
        );
        let postgres_aleph_bft_finalized_items = include_str!(
            "migrations/postgres/20260703165000_add_federation_aleph_bft_finalized_items.sql"
        );
        let sqlite_aleph_bft_backups =
            include_str!("migrations/sqlite/20260703170000_add_federation_aleph_bft_backups.sql");
        let postgres_aleph_bft_backups =
            include_str!("migrations/postgres/20260703170000_add_federation_aleph_bft_backups.sql");
        let sqlite_outcomes =
            include_str!("migrations/sqlite/20260703133000_add_federation_session_outcomes.sql");
        let postgres_outcomes =
            include_str!("migrations/postgres/20260703133000_add_federation_session_outcomes.sql");
        let sqlite_payment_observations = include_str!(
            "migrations/sqlite/20260703140000_add_federation_payment_observations.sql"
        );
        let postgres_payment_observations = include_str!(
            "migrations/postgres/20260703140000_add_federation_payment_observations.sql"
        );
        let sqlite_member_status =
            include_str!("migrations/sqlite/20260703162000_add_federation_member_status.sql");
        let postgres_member_status =
            include_str!("migrations/postgres/20260703162000_add_federation_member_status.sql");
        let sqlite_config =
            include_str!("migrations/sqlite/20260703162500_add_federation_config.sql");
        let postgres_config =
            include_str!("migrations/postgres/20260703162500_add_federation_config.sql");
        let sqlite_dkg_setup =
            include_str!("migrations/sqlite/20260703143000_add_federation_dkg_setup.sql");
        let postgres_dkg_setup =
            include_str!("migrations/postgres/20260703143000_add_federation_dkg_setup.sql");
        let sqlite_dkg_member_keyset_shares = include_str!(
            "migrations/sqlite/20260703150000_add_federation_dkg_member_keyset_shares.sql"
        );
        let postgres_dkg_member_keyset_shares = include_str!(
            "migrations/postgres/20260703150000_add_federation_dkg_member_keyset_shares.sql"
        );
        let sqlite_dkg_secret_share_contributions = include_str!(
            "migrations/sqlite/20260703154500_add_federation_dkg_secret_share_contributions.sql"
        );
        let postgres_dkg_secret_share_contributions = include_str!(
            "migrations/postgres/20260703154500_add_federation_dkg_secret_share_contributions.sql"
        );
        let sqlite_dkg_transcript_signatures = include_str!(
            "migrations/sqlite/20260703153000_add_federation_dkg_transcript_signatures.sql"
        );
        let postgres_dkg_transcript_signatures = include_str!(
            "migrations/postgres/20260703153000_add_federation_dkg_transcript_signatures.sql"
        );

        for migration in [sqlite_saga, postgres_saga] {
            assert_contains_all(
                migration,
                &["saga_state", "federation_operation_id", "TEXT"],
            );
        }

        for migration in [sqlite_journal, postgres_journal] {
            assert_contains_all(
                migration,
                &[
                    "federation_journal_entries",
                    "operation_index",
                    "operation_id",
                    "envelope_json",
                    "conflict_key_json",
                    "conflict_keys_json",
                    "previous_order_digest",
                    "order_digest",
                    "status",
                    "idx_federation_journal_operation_id",
                    "federation_member_sequences",
                    "member_id",
                    "last_sequence",
                ],
            );
        }

        for migration in [
            sqlite_journal_consensus_evidence,
            postgres_journal_consensus_evidence,
        ] {
            assert_contains_all(
                migration,
                &[
                    "federation_journal_entries",
                    "operation_outcome_hash",
                    "state_digest",
                    "consensus_evidence_json",
                ],
            );
        }

        for migration in [
            sqlite_aleph_bft_finalized_items,
            postgres_aleph_bft_finalized_items,
        ] {
            assert_contains_all(
                migration,
                &[
                    "federation_aleph_bft_finalized_items",
                    "accepted_item_index",
                    "session",
                    "unit_finalization_index",
                    "unit_submitting_member",
                    "unit_index",
                    "item_index_in_unit",
                    "operation_id",
                    "status",
                    "unit_body_hash",
                    "item_json",
                    "idx_federation_aleph_bft_finalized_items_session_unit",
                    "idx_federation_aleph_bft_finalized_items_operation_id",
                ],
            );
        }

        for migration in [sqlite_aleph_bft_backups, postgres_aleph_bft_backups] {
            assert_contains_all(
                migration,
                &["federation_aleph_bft_backups", "session", "backup_bytes"],
            );
        }

        for migration in [sqlite_outcomes, postgres_outcomes] {
            assert_contains_all(
                migration,
                &[
                    "federation_session_outcomes",
                    "from_index",
                    "signer_member_id",
                    "latest_index",
                    "operation_count",
                    "outcome_json",
                    "idx_federation_session_outcomes_from_index",
                ],
            );
        }

        for migration in [sqlite_payment_observations, postgres_payment_observations] {
            assert_contains_all(
                migration,
                &[
                    "federation_payment_observations",
                    "operation_id",
                    "operation_index",
                    "context",
                    "quote",
                    "observer_member_id",
                    "state",
                    "observation_json",
                    "idx_federation_payment_observations_member_state",
                    "idx_federation_payment_observations_quote",
                ],
            );
        }

        for migration in [sqlite_member_status, postgres_member_status] {
            assert_contains_all(
                migration,
                &[
                    "federation_member_status",
                    "member_id",
                    "status",
                    "peer_member_id",
                    "local_operation_count",
                    "peer_operation_count",
                    "operation_count",
                    "message",
                    "updated_time",
                ],
            );
        }

        for migration in [sqlite_config, postgres_config] {
            assert_contains_all(
                migration,
                &[
                    "federation_config",
                    "federation_id",
                    "config_digest",
                    "config_json",
                ],
            );
        }

        for migration in [sqlite_dkg_setup, postgres_dkg_setup] {
            assert_contains_all(
                migration,
                &[
                    "federation_dkg_setup_messages",
                    "phase",
                    "member_id",
                    "payload_json",
                    "idx_federation_dkg_setup_member",
                ],
            );
        }

        for migration in [
            sqlite_dkg_member_keyset_shares,
            postgres_dkg_member_keyset_shares,
        ] {
            assert_contains_all(
                migration,
                &[
                    "federation_dkg_member_keyset_shares",
                    "keyset_id",
                    "member_id",
                    "payload_json",
                    "idx_federation_dkg_member_keyset_shares_member",
                ],
            );
        }

        for migration in [
            sqlite_dkg_secret_share_contributions,
            postgres_dkg_secret_share_contributions,
        ] {
            assert_contains_all(
                migration,
                &[
                    "federation_dkg_secret_share_contributions",
                    "sender_id",
                    "receiver_id",
                    "payload_json",
                    "idx_federation_dkg_secret_share_contributions_receiver",
                ],
            );
        }

        for migration in [
            sqlite_dkg_transcript_signatures,
            postgres_dkg_transcript_signatures,
        ] {
            assert_contains_all(
                migration,
                &[
                    "federation_dkg_transcript_signatures",
                    "transcript_hash",
                    "member_id",
                    "payload_json",
                    "idx_federation_dkg_transcript_signatures_member",
                ],
            );
        }
    }

    #[test]
    fn melt_quote_state_schema_keeps_unknown_transient() {
        let postgres_initial = include_str!("migrations/postgres/1_initial.sql");
        let sqlite_quote_state = include_str!("migrations/sqlite/20240618195700_quote_state.sql");
        let sqlite_request_lookup_rebuild = include_str!(
            "migrations/sqlite/20250819200000_remove_request_lookup_kind_constraints.sql"
        );

        for sql in [
            postgres_initial,
            sqlite_quote_state,
            sqlite_request_lookup_rebuild,
        ] {
            assert_contains_all(sql, &["'UNPAID'", "'PENDING'", "'PAID'", "'FAILED'"]);
            let compact = compact_sql(sql);
            assert!(
                compact.contains("stateIN('UNPAID','PENDING','PAID','FAILED')"),
                "melt quote schema must admit FAILED in its durable-state constraint:\n{sql}"
            );
            assert!(
                !compact.contains("stateIN('UNPAID','PENDING','PAID','UNKNOWN'")
                    && !compact.contains("stateIN('UNPAID','PENDING','PAID','FAILED','UNKNOWN'")
                    && !compact.contains("stateIN('UNPAID','PENDING','PAID','UNKNOWN','FAILED'"),
                "melt quote schema must not persist backend Unknown as a durable state:\n{sql}"
            );
        }

        for (_, name, sql) in super::migrations::MIGRATIONS {
            assert_melt_quote_state_constraint_keeps_unknown_transient(name, sql);
            assert!(
                !name.contains("allow_failed_melt_quote_state"),
                "FAILED quote-state support should be folded into base schemas during federation development"
            );
            assert!(
                !name.contains("unknown_melt_quote_state"),
                "backend Unknown must remain transient and must not be introduced by a schema migration"
            );
        }
    }
}

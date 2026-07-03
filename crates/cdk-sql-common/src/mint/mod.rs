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
mod event_log;
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

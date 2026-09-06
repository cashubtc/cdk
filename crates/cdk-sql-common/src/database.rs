//! Database traits definition

use std::fmt::Debug;

use cdk_common::database::Error;

use crate::stmt::{Column, Statement};

/// Database Executor
///
/// This trait defines the expectations of a database execution
#[async_trait::async_trait]
pub trait DatabaseExecutor: Debug + Sync + Send {
    /// Database driver name
    fn name() -> &'static str;

    /// Executes a query and returns the affected rows
    async fn execute(&self, statement: Statement) -> Result<usize, Error>;

    /// Runs the query and returns the first row or None
    async fn fetch_one(&self, statement: Statement) -> Result<Option<Vec<Column>>, Error>;

    /// Runs the query and returns the first row or None
    async fn fetch_all(&self, statement: Statement) -> Result<Vec<Vec<Column>>, Error>;

    /// Fetches the first row and column from a query
    async fn pluck(&self, statement: Statement) -> Result<Option<Column>, Error>;

    /// Batch execution
    async fn batch(&self, statement: Statement) -> Result<(), Error>;
}

/// An owned SQL transaction. The backend is responsible for rollback on drop.
#[async_trait::async_trait]
pub trait SqlTransaction: DatabaseExecutor + Sized + 'static {
    /// Commit and release this transaction's connection.
    async fn commit(self) -> Result<(), Error>;

    /// Roll back and release this transaction's connection.
    async fn rollback(self) -> Result<(), Error>;
}

/// An owned connection checked out from a library-managed pool.
#[async_trait::async_trait]
pub trait SqlConnection: DatabaseExecutor + Sized + 'static {
    /// Transaction owning this connection.
    type Transaction: SqlTransaction;

    /// Begin a transaction without acquiring another connection.
    async fn begin_transaction(self) -> Result<Self::Transaction, Error>;
}

/// Backend adapter for a library-managed SQL pool.
///
/// Implementations delegate all connection capacity, waiting, and recycling to
/// their pool library. This interface only connects that library to CDK's SQL.
#[async_trait::async_trait]
pub trait SqlBackend: Clone + Debug + Send + Sync + 'static {
    /// Backend configuration accepted by database constructors.
    type Config;
    /// Owned connection returned by the pool adapter.
    type Connection: SqlConnection<Transaction = Self::Transaction>;
    /// Owned transaction returned by the backend.
    type Transaction: SqlTransaction;

    /// Construct the library pool, validating configuration without connecting.
    fn new(config: Self::Config) -> Result<Self, Error>;

    /// Acquire an owned connection using the library's timeout policy.
    async fn acquire(&self) -> Result<Self::Connection, Error>;

    /// Acquire a connection and begin a transaction.
    async fn begin_transaction(&self) -> Result<Self::Transaction, Error> {
        self.acquire().await?.begin_transaction().await
    }

    /// Begin a transaction for bootstrap and migrations.
    async fn begin_migration(&self) -> Result<Self::Transaction, Error> {
        self.begin_transaction().await
    }
}

/// Metrics for the lifetime of a successful connection checkout.
///
/// Move this guard with the connection into asynchronous cleanup so cleanup is
/// included in the checkout duration. It does not manage pooled resources.
#[derive(Debug, Default)]
pub struct ConnectionMetrics {
    #[cfg(feature = "prometheus")]
    started: Option<std::time::Instant>,
}

impl ConnectionMetrics {
    /// Record a successful checkout.
    pub fn acquired() -> Self {
        #[cfg(feature = "prometheus")]
        {
            cdk_prometheus::METRICS.increment_db_connections_active();
            Self {
                started: Some(std::time::Instant::now()),
            }
        }
        #[cfg(not(feature = "prometheus"))]
        Self {}
    }
}

impl Drop for ConnectionMetrics {
    fn drop(&mut self) {
        #[cfg(feature = "prometheus")]
        if let Some(started) = self.started.take() {
            cdk_prometheus::METRICS.decrement_db_connections_active();
            cdk_prometheus::METRICS.record_db_operation(started.elapsed().as_secs_f64(), "drop");
        }
    }
}

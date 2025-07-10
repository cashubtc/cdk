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

/// Database transaction trait
#[async_trait::async_trait]
pub trait DatabaseTransaction<'a>: Debug + DatabaseExecutor + Send + Sync {
    /// Consumes the current transaction committing the changes
    async fn commit(self) -> Result<(), Error>;

    /// Consumes the transaction rolling back all changes
    async fn rollback(self) -> Result<(), Error>;
}

/// Database connector
#[async_trait::async_trait]
pub trait DatabaseConnector: Debug + DatabaseExecutor + Send + Sync {
    /// Transaction type for this database connection
    type Transaction<'a>: DatabaseTransaction<'a>
    where
        Self: 'a;

    /// Begin a new transaction
    async fn begin(&self) -> Result<Self::Transaction<'_>, Error>;
}

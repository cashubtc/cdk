//! Database traits definition

use std::fmt::Debug;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

use cdk_common::database::Error;

use crate::stmt::{query, Column, Statement};

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
pub trait DatabaseTransaction<DB>
where
    DB: DatabaseExecutor,
{
    /// Consumes the current transaction committing the changes
    async fn commit(conn: &mut DB) -> Result<(), Error>;

    /// Begin a transaction
    async fn begin(conn: &mut DB) -> Result<(), Error>;

    /// Consumes the transaction rolling back all changes
    async fn rollback(conn: &mut DB) -> Result<(), Error>;
}

/// Database connection with a transaction
#[derive(Debug)]
pub struct ConnectionWithTransaction<DB, W>
where
    DB: DatabaseConnector + 'static,
    W: Debug + Deref<Target = DB> + DerefMut<Target = DB> + Send + Sync + 'static,
{
    inner: Option<W>,
}

impl<DB, W> ConnectionWithTransaction<DB, W>
where
    DB: DatabaseConnector,
    W: Debug + Deref<Target = DB> + DerefMut<Target = DB> + Send + Sync + 'static,
{
    /// Creates a new transaction
    pub async fn new(mut inner: W) -> Result<Self, Error> {
        DB::Transaction::begin(inner.deref_mut()).await?;
        Ok(Self { inner: Some(inner) })
    }

    /// Commits the transaction consuming it and releasing the connection back to the pool (or
    /// disconnecting)
    pub async fn commit(mut self) -> Result<(), Error> {
        let mut conn = self
            .inner
            .take()
            .ok_or(Error::Internal("Missing connection".to_owned()))?;

        DB::Transaction::commit(&mut conn).await?;

        Ok(())
    }

    /// Rollback the transaction consuming it and releasing the connection back to the pool (or
    /// disconnecting)
    pub async fn rollback(mut self) -> Result<(), Error> {
        let mut conn = self
            .inner
            .take()
            .ok_or(Error::Internal("Missing connection".to_owned()))?;

        DB::Transaction::rollback(&mut conn).await?;

        Ok(())
    }
}

impl<DB, W> Drop for ConnectionWithTransaction<DB, W>
where
    DB: DatabaseConnector,
    W: Debug + Deref<Target = DB> + DerefMut<Target = DB> + Send + Sync + 'static,
{
    fn drop(&mut self) {
        if let Some(mut conn) = self.inner.take() {
            tokio::spawn(async move {
                let _ = DB::Transaction::rollback(conn.deref_mut()).await;
            });
        }
    }
}

#[async_trait::async_trait]
impl<DB, W> DatabaseExecutor for ConnectionWithTransaction<DB, W>
where
    DB: DatabaseConnector,
    W: Debug + Deref<Target = DB> + DerefMut<Target = DB> + Send + Sync + 'static,
{
    fn name() -> &'static str {
        "Transaction"
    }

    /// Executes a query and returns the affected rows
    async fn execute(&self, statement: Statement) -> Result<usize, Error> {
        self.inner
            .as_ref()
            .ok_or(Error::Internal("Missing internal connection".to_owned()))?
            .execute(statement)
            .await
    }

    /// Runs the query and returns the first row or None
    async fn fetch_one(&self, statement: Statement) -> Result<Option<Vec<Column>>, Error> {
        self.inner
            .as_ref()
            .ok_or(Error::Internal("Missing internal connection".to_owned()))?
            .fetch_one(statement)
            .await
    }

    /// Runs the query and returns the first row or None
    async fn fetch_all(&self, statement: Statement) -> Result<Vec<Vec<Column>>, Error> {
        self.inner
            .as_ref()
            .ok_or(Error::Internal("Missing internal connection".to_owned()))?
            .fetch_all(statement)
            .await
    }

    /// Fetches the first row and column from a query
    async fn pluck(&self, statement: Statement) -> Result<Option<Column>, Error> {
        self.inner
            .as_ref()
            .ok_or(Error::Internal("Missing internal connection".to_owned()))?
            .pluck(statement)
            .await
    }

    /// Batch execution
    async fn batch(&self, statement: Statement) -> Result<(), Error> {
        self.inner
            .as_ref()
            .ok_or(Error::Internal("Missing internal connection".to_owned()))?
            .batch(statement)
            .await
    }
}

/// Generic transaction handler for SQLite
pub struct GenericTransactionHandler<W>(PhantomData<W>);

#[async_trait::async_trait]
impl<W> DatabaseTransaction<W> for GenericTransactionHandler<W>
where
    W: DatabaseExecutor,
{
    /// Consumes the current transaction committing the changes
    async fn commit(conn: &mut W) -> Result<(), Error> {
        query("COMMIT")?.execute(conn).await?;
        Ok(())
    }

    /// Begin a transaction
    async fn begin(conn: &mut W) -> Result<(), Error> {
        query("START TRANSACTION")?.execute(conn).await?;
        Ok(())
    }

    /// Consumes the transaction rolling back all changes
    async fn rollback(conn: &mut W) -> Result<(), Error> {
        query("ROLLBACK")?.execute(conn).await?;
        Ok(())
    }
}

/// Database connector
#[async_trait::async_trait]
pub trait DatabaseConnector: Debug + DatabaseExecutor + Send + Sync {
    /// Database static trait for the database
    type Transaction: DatabaseTransaction<Self>
    where
        Self: Sized;
}

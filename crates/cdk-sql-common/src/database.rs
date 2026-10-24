//! Database traits definition

use std::fmt::Debug;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cdk_common::database::Error;

use crate::pool::{DatabasePool, PooledResource};
use crate::stmt::{query, Column, Statement};

/// Upper bound for best-effort rollback of a dropped transaction. A rollback that
/// does not finish in time discards the connection instead of holding its pool slot.
const ROLLBACK_TIMEOUT: Duration = Duration::from_secs(5);

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
    stale: Arc<AtomicBool>,
}

impl<DB, RM> ConnectionWithTransaction<DB, PooledResource<RM>>
where
    DB: DatabaseConnector + 'static,
    RM: DatabasePool<Connection = DB> + 'static,
{
    /// Creates a transaction using the pooled connection's existing stale flag.
    pub async fn new(inner: PooledResource<RM>) -> Result<Self, Error> {
        // Own the connection before awaiting BEGIN: cancellation can leave the
        // database operation running and it may still succeed. Drop must retain
        // the connection until rollback completes before returning it to the pool.
        let mut transaction = Self {
            stale: inner.stale(),
            inner: Some(inner),
        };
        let conn = transaction
            .inner
            .as_mut()
            .ok_or(Error::Internal("Missing connection".to_owned()))?;
        DB::Transaction::begin(conn.deref_mut()).await?;
        Ok(transaction)
    }
}

impl<DB, W> ConnectionWithTransaction<DB, W>
where
    DB: DatabaseConnector,
    W: Debug + Deref<Target = DB> + DerefMut<Target = DB> + Send + Sync + 'static,
{
    /// Commits the transaction consuming it and releasing the connection back to the pool (or
    /// disconnecting)
    pub async fn commit(mut self) -> Result<(), Error> {
        let conn = self
            .inner
            .as_mut()
            .ok_or(Error::Internal("Missing connection".to_owned()))?;

        DB::Transaction::commit(conn.deref_mut()).await?;
        self.inner.take();

        Ok(())
    }

    /// Rollback the transaction consuming it and releasing the connection back to the pool (or
    /// disconnecting)
    pub async fn rollback(mut self) -> Result<(), Error> {
        let conn = self
            .inner
            .as_mut()
            .ok_or(Error::Internal("Missing connection".to_owned()))?;

        DB::Transaction::rollback(conn.deref_mut()).await?;
        self.inner.take();

        Ok(())
    }
}

impl<DB, W> Drop for ConnectionWithTransaction<DB, W>
where
    DB: DatabaseConnector,
    W: Debug + Deref<Target = DB> + DerefMut<Target = DB> + Send + Sync + 'static,
{
    fn drop(&mut self) {
        if let Some(conn) = self.inner.take() {
            // Construct before spawning so even an unpolled cleanup future
            // owns the guard that fences failed or cancelled rollback.
            let cleanup = RollbackGuard {
                inner: Some(conn),
                stale: self.stale.clone(),
            };
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(cleanup.rollback());
            }
        }
    }
}

struct RollbackGuard<DB, W>
where
    DB: DatabaseConnector + 'static,
    W: Debug + Deref<Target = DB> + DerefMut<Target = DB> + Send + Sync + 'static,
{
    inner: Option<W>,
    stale: Arc<AtomicBool>,
}

impl<DB, W> RollbackGuard<DB, W>
where
    DB: DatabaseConnector,
    W: Debug + Deref<Target = DB> + DerefMut<Target = DB> + Send + Sync + 'static,
{
    async fn rollback(mut self) {
        if let Some(conn) = self.inner.as_mut() {
            if let Ok(Ok(())) = tokio::time::timeout(
                ROLLBACK_TIMEOUT,
                DB::Transaction::rollback(conn.deref_mut()),
            )
            .await
            {
                self.inner.take();
            }
        }
    }
}

impl<DB, W> Drop for RollbackGuard<DB, W>
where
    DB: DatabaseConnector,
    W: Debug + Deref<Target = DB> + DerefMut<Target = DB> + Send + Sync + 'static,
{
    fn drop(&mut self) {
        if self.inner.is_some() {
            self.stale.store(true, Ordering::SeqCst);
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
#[allow(missing_debug_implementations)]
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

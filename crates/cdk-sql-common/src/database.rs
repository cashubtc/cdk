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
///
/// The resource is not reusable until a transaction-ending command succeeds.
#[derive(Debug)]
pub struct ConnectionWithTransaction<DB, W>
where
    DB: DatabaseConnector + 'static,
    W: Debug
        + Deref<Target = DB>
        + DerefMut<Target = DB>
        + TransactionResource
        + Send
        + Sync
        + 'static,
{
    inner: Option<W>,
    begun: bool,
}

impl<DB, W> ConnectionWithTransaction<DB, W>
where
    DB: DatabaseConnector,
    W: Debug
        + Deref<Target = DB>
        + DerefMut<Target = DB>
        + TransactionResource
        + Send
        + Sync
        + 'static,
{
    /// Creates a new transaction
    pub async fn new(mut inner: W) -> Result<Self, Error> {
        inner.set_reusable(false);
        let mut transaction = Self {
            inner: Some(inner),
            begun: false,
        };
        let conn = transaction
            .inner
            .as_mut()
            .ok_or(Error::Internal("Missing connection".to_owned()))?;
        DB::Transaction::begin(conn.deref_mut()).await?;
        transaction.begun = true;
        Ok(transaction)
    }

    /// Commits the transaction consuming it and releasing the connection back to the pool (or
    /// disconnecting)
    pub async fn commit(mut self) -> Result<(), Error> {
        let conn = self
            .inner
            .as_mut()
            .ok_or(Error::Internal("Missing connection".to_owned()))?;

        DB::Transaction::commit(conn.deref_mut()).await?;
        conn.set_reusable(true);
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

        let result = DB::Transaction::rollback(conn.deref_mut()).await;
        if result.is_ok() {
            conn.set_reusable(true);
        }
        self.inner.take();
        result
    }
}

impl<DB, W> Drop for ConnectionWithTransaction<DB, W>
where
    DB: DatabaseConnector,
    W: Debug
        + Deref<Target = DB>
        + DerefMut<Target = DB>
        + TransactionResource
        + Send
        + Sync
        + 'static,
{
    fn drop(&mut self) {
        if let Some(mut conn) = self.inner.take() {
            if !self.begun {
                return;
            }
            // Cleanup is best effort. The resource remains non-reusable on error,
            // timeout, task cancellation, or absence of a runtime.
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    if matches!(
                        tokio::time::timeout(
                            std::time::Duration::from_secs(5),
                            DB::Transaction::rollback(conn.deref_mut()),
                        )
                        .await,
                        Ok(Ok(()))
                    ) {
                        conn.set_reusable(true);
                    }
                });
            }
        }
    }
}

#[async_trait::async_trait]
impl<DB, W> DatabaseExecutor for ConnectionWithTransaction<DB, W>
where
    DB: DatabaseConnector,
    W: Debug
        + Deref<Target = DB>
        + DerefMut<Target = DB>
        + TransactionResource
        + Send
        + Sync
        + 'static,
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

/// Exclusive connection ownership used by a transaction guard.
///
/// Implementations must close/discard the connection when dropped with reuse
/// disabled, including during unwinding or cancellation. Only an acknowledged
/// commit or rollback may re-enable reuse; driver health flags remain independent.
pub trait TransactionResource {
    /// Allow pooling only after successful transaction cleanup.
    fn set_reusable(&mut self, reusable: bool);
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use tokio::sync::Notify;

    use super::*;

    #[derive(Debug, Default)]
    struct TransactionState {
        active: AtomicBool,
        returned_active: AtomicBool,
        reusable: AtomicBool,
        closed_active: AtomicBool,
        wait_for_commit: AtomicBool,
        fail_rollback_once: AtomicBool,
        wait_for_rollback_once: AtomicBool,
        commit_entered: Notify,
        rollback_entered: Notify,
        returned: Notify,
    }

    #[derive(Debug)]
    struct TestConnection(Arc<TransactionState>);

    impl TransactionResource for Box<TestConnection> {
        fn set_reusable(&mut self, reusable: bool) {
            self.0.reusable.store(reusable, Ordering::SeqCst);
        }
    }

    impl Drop for TestConnection {
        fn drop(&mut self) {
            let active = self.0.active.load(Ordering::SeqCst);
            let reusable = self.0.reusable.load(Ordering::SeqCst);
            self.0
                .returned_active
                .store(active && reusable, Ordering::SeqCst);
            self.0
                .closed_active
                .store(active && !reusable, Ordering::SeqCst);
            self.0.returned.notify_one();
        }
    }

    #[async_trait::async_trait]
    impl DatabaseExecutor for TestConnection {
        fn name() -> &'static str {
            "transaction-test"
        }

        async fn execute(&self, _: Statement) -> Result<usize, Error> {
            unreachable!()
        }

        async fn fetch_one(&self, _: Statement) -> Result<Option<Vec<Column>>, Error> {
            unreachable!()
        }

        async fn fetch_all(&self, _: Statement) -> Result<Vec<Vec<Column>>, Error> {
            unreachable!()
        }

        async fn pluck(&self, _: Statement) -> Result<Option<Column>, Error> {
            unreachable!()
        }

        async fn batch(&self, _: Statement) -> Result<(), Error> {
            unreachable!()
        }
    }

    #[async_trait::async_trait]
    impl DatabaseConnector for TestConnection {
        type Transaction = Self;
    }

    #[async_trait::async_trait]
    impl DatabaseTransaction<TestConnection> for TestConnection {
        async fn begin(conn: &mut Self) -> Result<(), Error> {
            conn.0.active.store(true, Ordering::SeqCst);
            Ok(())
        }

        async fn commit(conn: &mut Self) -> Result<(), Error> {
            conn.0.commit_entered.notify_one();
            if conn.0.wait_for_commit.load(Ordering::SeqCst) {
                std::future::pending::<()>().await;
            }
            Err(Error::Internal("injected commit failure".to_owned()))
        }

        async fn rollback(conn: &mut Self) -> Result<(), Error> {
            if conn.0.fail_rollback_once.swap(false, Ordering::SeqCst) {
                return Err(Error::Internal("injected rollback failure".to_owned()));
            }
            if conn.0.wait_for_rollback_once.swap(false, Ordering::SeqCst) {
                conn.0.rollback_entered.notify_one();
                std::future::pending::<()>().await;
            }
            conn.0.active.store(false, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn failed_commit_rolls_back_before_returning_connection() {
        let state = Arc::new(TransactionState::default());
        let transaction = ConnectionWithTransaction::new(Box::new(TestConnection(state.clone())))
            .await
            .expect("test transaction begins");
        assert!(transaction.commit().await.is_err());
        state.returned.notified().await;
        assert!(!state.returned_active.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn cancelled_commit_rolls_back_before_returning_connection() {
        let state = Arc::new(TransactionState::default());
        state.wait_for_commit.store(true, Ordering::SeqCst);
        let transaction = ConnectionWithTransaction::new(Box::new(TestConnection(state.clone())))
            .await
            .expect("test transaction begins");
        let commit = tokio::spawn(transaction.commit());
        state.commit_entered.notified().await;
        commit.abort();
        assert!(commit.await.expect_err("commit was aborted").is_cancelled());
        state.returned.notified().await;
        assert!(!state.returned_active.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn failed_rollback_discards_and_cancelled_rollback_keeps_cleanup_ownership() {
        for cancel in [false, true] {
            let state = Arc::new(TransactionState::default());
            state.fail_rollback_once.store(!cancel, Ordering::SeqCst);
            state.wait_for_rollback_once.store(cancel, Ordering::SeqCst);
            let transaction =
                ConnectionWithTransaction::new(Box::new(TestConnection(state.clone())))
                    .await
                    .expect("test transaction begins");
            match cancel {
                false => assert!(transaction.rollback().await.is_err()),
                true => {
                    let rollback = tokio::spawn(transaction.rollback());
                    state.rollback_entered.notified().await;
                    rollback.abort();
                    assert!(rollback
                        .await
                        .expect_err("rollback was aborted")
                        .is_cancelled());
                }
            }
            state.returned.notified().await;
            assert!(!state.returned_active.load(Ordering::SeqCst));
            assert_eq!(state.closed_active.load(Ordering::SeqCst), !cancel);
        }
    }
}

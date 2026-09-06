use std::sync::Arc;
use std::time::Duration;

use cdk_common::database::Error;
use cdk_sql_common::database::{
    ConnectionMetrics, DatabaseExecutor, SqlConnection, SqlTransaction,
};
use cdk_sql_common::stmt::{Column, Statement};
use deadpool_sqlite::Object;
use tokio::sync::{oneshot, Mutex};

use super::async_sqlite;
use super::backend::{database_error, BackendInner};

#[derive(Debug, thiserror::Error)]
enum ConnectionError {
    #[error("SQLite connection was consumed by a cancelled operation")]
    Consumed,
    #[error("SQLite blocking operation panicked")]
    WorkerPanic,
    #[error("SQLite blocking operation was aborted")]
    WorkerAborted,
}

#[derive(Debug)]
struct Lease {
    object: Option<Object>,
    dirty: bool,
    pending: Option<oneshot::Receiver<()>>,
    owner: Arc<BackendInner>,
    metrics: Option<ConnectionMetrics>,
}

impl Lease {
    fn object(&self) -> &Object {
        self.object
            .as_ref()
            .expect("lease owns its connection until drop")
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if !self.dirty {
            let _entered = self.owner.runtime.enter();
            drop(self.object.take());
            return;
        }
        if let Some(object) = self.object.take() {
            let mut cleanup = Cleanup {
                object: Some(object),
                pending: self.pending.take(),
                owner: self.owner.clone(),
                _metrics: self.metrics.take(),
            };
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let result = tokio::time::timeout(Duration::from_secs(10), async {
                        // interact cancellation does not cancel its blocking
                        // closure. Wait for that closure before rolling back,
                        // including when it has not started running yet.
                        if let Some(pending) = cleanup.pending.take() {
                            let _ = pending.await;
                        }
                        cleanup
                            .object()
                            .interact(|conn| {
                                if conn.is_autocommit() {
                                    Ok(())
                                } else {
                                    conn.execute_batch("ROLLBACK")
                                }
                            })
                            .await
                    })
                    .await;
                    match result {
                        Ok(Ok(Ok(()))) => {
                            drop(cleanup.object.take());
                        }
                        _ => tracing::warn!(
                            "Discarding SQLite connection after unsuccessful rollback"
                        ),
                    }
                });
            }
        }
    }
}

struct Cleanup {
    object: Option<Object>,
    pending: Option<oneshot::Receiver<()>>,
    owner: Arc<BackendInner>,
    _metrics: Option<ConnectionMetrics>,
}

impl Cleanup {
    fn object(&self) -> &Object {
        self.object.as_ref().expect("cleanup owns its connection")
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(object) = self.object.take() {
            let _entered = self.owner.runtime.enter();
            self.owner.discard_memory();
            drop(Object::take(object));
        }
    }
}

/// Owned SQLite connection whose blocking work runs through Deadpool.
#[derive(Debug)]
pub struct SqliteConnection {
    lease: Mutex<Option<Lease>>,
    in_transaction: bool,
}

impl SqliteConnection {
    pub(crate) fn new(object: Object, owner: Arc<BackendInner>) -> Self {
        Self {
            lease: Mutex::new(Some(Lease {
                object: Some(object),
                dirty: false,
                pending: None,
                owner,
                metrics: Some(ConnectionMetrics::acquired()),
            })),
            in_transaction: false,
        }
    }

    async fn run<T, F>(&self, operation: F) -> Result<T, Error>
    where
        T: Send + 'static,
        F: FnOnce(&mut rusqlite::Connection) -> Result<T, Error> + Send + 'static,
    {
        let mut slot = self.lease.lock().await;
        let mut lease = slot
            .take()
            .ok_or_else(|| database_error(ConnectionError::Consumed))?;
        lease.dirty = true;
        let (finished, pending) = oneshot::channel();
        lease.pending = Some(pending);
        let result = lease
            .object()
            .interact(move |conn| {
                // Dropping the sender signals completion even when operation panics.
                let _finished = finished;
                operation(conn)
            })
            .await;
        lease.pending = None;
        let result = result.map_err(|error| {
            database_error(match error {
                deadpool_sqlite::InteractError::Panic(_) => ConnectionError::WorkerPanic,
                deadpool_sqlite::InteractError::Aborted => ConnectionError::WorkerAborted,
            })
        })?;
        lease.dirty = self.in_transaction;
        *slot = Some(lease);
        result
    }

    async fn finish(self, command: &'static str) -> Result<(), Error> {
        self.run(move |conn| conn.execute_batch(command).map_err(database_error))
            .await?;
        if let Some(lease) = self.lease.lock().await.as_mut() {
            lease.dirty = false;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl SqlConnection for SqliteConnection {
    type Transaction = SqliteTransaction;
    async fn begin_transaction(mut self) -> Result<Self::Transaction, Error> {
        self.in_transaction = true;
        self.run(|conn| {
            conn.execute_batch("BEGIN IMMEDIATE")
                .map_err(database_error)
        })
        .await?;
        Ok(SqliteTransaction(self))
    }
}

#[async_trait::async_trait]
impl DatabaseExecutor for SqliteConnection {
    fn name() -> &'static str {
        "sqlite"
    }
    async fn execute(&self, statement: Statement) -> Result<usize, Error> {
        self.run(move |c| async_sqlite::execute(c, statement)).await
    }
    async fn fetch_one(&self, statement: Statement) -> Result<Option<Vec<Column>>, Error> {
        self.run(move |c| async_sqlite::fetch_one(c, statement))
            .await
    }
    async fn fetch_all(&self, statement: Statement) -> Result<Vec<Vec<Column>>, Error> {
        self.run(move |c| async_sqlite::fetch_all(c, statement))
            .await
    }
    async fn pluck(&self, statement: Statement) -> Result<Option<Column>, Error> {
        self.run(move |c| async_sqlite::pluck(c, statement)).await
    }
    async fn batch(&self, statement: Statement) -> Result<(), Error> {
        self.run(move |c| async_sqlite::batch(c, statement)).await
    }
}

/// SQLite transaction retaining its checkout until finalization or cleanup.
#[derive(Debug)]
pub struct SqliteTransaction(SqliteConnection);

#[async_trait::async_trait]
impl SqlTransaction for SqliteTransaction {
    async fn commit(self) -> Result<(), Error> {
        self.0.finish("COMMIT").await
    }
    async fn rollback(self) -> Result<(), Error> {
        self.0.finish("ROLLBACK").await
    }
}

#[async_trait::async_trait]
impl DatabaseExecutor for SqliteTransaction {
    fn name() -> &'static str {
        "sqlite"
    }
    async fn execute(&self, statement: Statement) -> Result<usize, Error> {
        self.0.execute(statement).await
    }
    async fn fetch_one(&self, statement: Statement) -> Result<Option<Vec<Column>>, Error> {
        self.0.fetch_one(statement).await
    }
    async fn fetch_all(&self, statement: Statement) -> Result<Vec<Vec<Column>>, Error> {
        self.0.fetch_all(statement).await
    }
    async fn pluck(&self, statement: Statement) -> Result<Option<Column>, Error> {
        self.0.pluck(statement).await
    }
    async fn batch(&self, statement: Statement) -> Result<(), Error> {
        self.0.batch(statement).await
    }
}

#[cfg(test)]
mod tests;

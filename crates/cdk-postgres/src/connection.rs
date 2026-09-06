use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use cdk_common::database::Error;
use cdk_sql_common::database::{
    CleanupOutcome, ConnectionMetrics, DatabaseExecutor, SqlConnection, SqlTransaction,
};
use cdk_sql_common::stmt::{Column, Statement};
use deadpool_postgres::Object;
use tokio::sync::Mutex;
use tokio_postgres::Client;

use super::backend::database_error;
use super::db::{pg_batch, pg_execute, pg_fetch_all, pg_fetch_one, pg_pluck};

#[derive(Debug, thiserror::Error)]
enum ConnectionError {
    #[error("PostgreSQL connection was consumed by a cancelled operation")]
    Consumed,
}

// This owns one library checkout, not a pool. Dirty checkouts cannot reach
// Deadpool's return-on-drop path until rollback has been acknowledged.
#[derive(Debug)]
struct Lease {
    object: Option<Object>,
    dirty: bool,
    timeout: Duration,
    metrics: Option<ConnectionMetrics>,
}

impl Lease {
    fn client(&self) -> &Client {
        self.object
            .as_ref()
            .expect("lease owns its connection until drop")
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if !self.dirty {
            return;
        }
        if let Some(object) = self.object.take() {
            let mut cleanup = Cleanup {
                object: Some(object),
                _metrics: self.metrics.take(),
                outcome: CleanupOutcome::RuntimeUnavailable,
            };
            let timeout = self.timeout;
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                cleanup.outcome = CleanupOutcome::TaskCancelled;
                runtime.spawn(async move {
                    let result =
                        tokio::time::timeout(timeout, cleanup.client().batch_execute("ROLLBACK"))
                            .await;
                    match result {
                        Ok(Ok(())) => {
                            CleanupOutcome::RolledBack.record("postgres");
                            drop(cleanup.object.take());
                        }
                        Ok(Err(_)) => cleanup.outcome = CleanupOutcome::RollbackError,
                        Err(_) => cleanup.outcome = CleanupOutcome::Timeout,
                    }
                    drop(cleanup);
                });
            }
            // Without a runtime, or if the task is cancelled, Cleanup detaches.
        }
    }
}

struct Cleanup {
    object: Option<Object>,
    _metrics: Option<ConnectionMetrics>,
    outcome: CleanupOutcome,
}

impl Cleanup {
    fn client(&self) -> &Client {
        self.object.as_ref().expect("cleanup owns its connection")
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(object) = self.object.take() {
            self.outcome.record("postgres");
            drop(Object::take(object));
        }
    }
}

/// Owned connection checked out from Deadpool.
#[derive(Debug)]
pub struct PostgresConnection {
    lease: Mutex<Option<Lease>>,
    schema: Option<String>,
    in_transaction: bool,
}

impl PostgresConnection {
    pub(crate) fn new(object: Object, schema: Option<String>, timeout: Duration) -> Self {
        Self {
            lease: Mutex::new(Some(Lease {
                object: Some(object),
                dirty: false,
                timeout,
                metrics: Some(ConnectionMetrics::acquired()),
            })),
            schema,
            in_transaction: false,
        }
    }

    async fn select_schema(client: &Client, schema: Option<&str>) -> Result<(), Error> {
        if let Some(schema) = schema {
            client
                .query_one(
                    "SELECT set_config('search_path', quote_ident($1), true)",
                    &[&schema],
                )
                .await
                .map_err(database_error)?;
        }
        Ok(())
    }

    async fn begin(mut self, migration: bool) -> Result<PostgresTransaction, Error> {
        {
            let slot = self.lease.get_mut();
            let lease = slot
                .as_mut()
                .ok_or_else(|| database_error(ConnectionError::Consumed))?;
            lease.dirty = true;
            lease
                .client()
                .batch_execute("BEGIN")
                .await
                .map_err(database_error)?;
            if migration {
                // Include the database and resolved schema in the lock key. This
                // also serializes concurrent CREATE SCHEMA and migration runners.
                lease.client().query_one(
                    "SELECT pg_advisory_xact_lock(hashtextextended('cdk:migrations:' || current_database() || ':' || COALESCE($1::text, current_schema()), 0))",
                    &[&self.schema],
                ).await.map_err(database_error)?;
                if let Some(schema) = self.schema.as_ref() {
                    let identifier = schema.replace('"', "\"\"");
                    lease
                        .client()
                        .batch_execute(&format!("CREATE SCHEMA IF NOT EXISTS \"{identifier}\""))
                        .await
                        .map_err(database_error)?;
                }
            }
            Self::select_schema(lease.client(), self.schema.as_deref()).await?;
        }
        self.in_transaction = true;
        Ok(PostgresTransaction(self))
    }

    pub(crate) async fn begin_migration(self) -> Result<PostgresTransaction, Error> {
        self.begin(true).await
    }

    async fn run<T, F>(&self, operation: F) -> Result<T, Error>
    where
        T: Send,
        F: for<'a> FnOnce(
                &'a Client,
            )
                -> Pin<Box<dyn Future<Output = Result<T, Error>> + Send + 'a>>
            + Send,
    {
        let mut slot = self.lease.lock().await;
        // Taking the lease means cancellation leaves this adapter unusable and
        // transfers its connection to rollback, even when the adapter survives.
        let mut lease = slot
            .take()
            .ok_or_else(|| database_error(ConnectionError::Consumed))?;
        lease.dirty = true;
        let scoped = !self.in_transaction && self.schema.is_some();
        if scoped {
            lease
                .client()
                .batch_execute("BEGIN")
                .await
                .map_err(database_error)?;
            Self::select_schema(lease.client(), self.schema.as_deref()).await?;
        }
        let result = operation(lease.client()).await;
        if scoped {
            match result.as_ref() {
                Ok(_) => lease
                    .client()
                    .batch_execute("COMMIT")
                    .await
                    .map_err(database_error)?,
                Err(_) => lease
                    .client()
                    .batch_execute("ROLLBACK")
                    .await
                    .map_err(database_error)?,
            }
        }
        lease.dirty = self.in_transaction;
        *slot = Some(lease);
        result
    }
}

#[async_trait::async_trait]
impl SqlConnection for PostgresConnection {
    type Transaction = PostgresTransaction;
    async fn begin_transaction(self) -> Result<Self::Transaction, Error> {
        self.begin(false).await
    }
}

#[async_trait::async_trait]
impl DatabaseExecutor for PostgresConnection {
    fn name() -> &'static str {
        "postgres"
    }
    async fn execute(&self, statement: Statement) -> Result<usize, Error> {
        self.run(move |c| Box::pin(pg_execute(c, statement))).await
    }
    async fn fetch_one(&self, statement: Statement) -> Result<Option<Vec<Column>>, Error> {
        self.run(move |c| Box::pin(pg_fetch_one(c, statement)))
            .await
    }
    async fn fetch_all(&self, statement: Statement) -> Result<Vec<Vec<Column>>, Error> {
        self.run(move |c| Box::pin(pg_fetch_all(c, statement)))
            .await
    }
    async fn pluck(&self, statement: Statement) -> Result<Option<Column>, Error> {
        self.run(move |c| Box::pin(pg_pluck(c, statement))).await
    }
    async fn batch(&self, statement: Statement) -> Result<(), Error> {
        self.run(move |c| Box::pin(pg_batch(c, statement))).await
    }
}

/// Transaction retaining its Deadpool checkout through finalization or cleanup.
#[derive(Debug)]
pub struct PostgresTransaction(PostgresConnection);

impl PostgresTransaction {
    async fn finish(mut self, command: &str) -> Result<(), Error> {
        let mut lease = self
            .0
            .lease
            .get_mut()
            .take()
            .ok_or_else(|| database_error(ConnectionError::Consumed))?;
        lease
            .client()
            .batch_execute(command)
            .await
            .map_err(database_error)?;
        lease.dirty = false;
        Ok(())
    }
}

#[async_trait::async_trait]
impl SqlTransaction for PostgresTransaction {
    async fn commit(self) -> Result<(), Error> {
        self.finish("COMMIT").await
    }
    async fn rollback(self) -> Result<(), Error> {
        self.finish("ROLLBACK").await
    }
}

#[async_trait::async_trait]
impl DatabaseExecutor for PostgresTransaction {
    fn name() -> &'static str {
        "postgres"
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

//! Simple SQLite
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use cdk_common::database::Error;
use cdk_sql_common::database::{DatabaseConnector, DatabaseExecutor, DatabaseTransaction};
use cdk_sql_common::run_db_operation_sync;
use cdk_sql_common::stmt::{query, Column, SqlPart, Statement};
use rusqlite::{ffi, CachedStatement, Connection, Error as SqliteError, ErrorCode};
use tokio::sync::Mutex;

use crate::common::{from_sqlite, to_sqlite};

/// Async Sqlite wrapper
#[derive(Debug)]
pub struct AsyncSqlite {
    inner: Arc<Mutex<Connection>>,
    stale: Arc<AtomicBool>,
    #[cfg(test)]
    hooks: Arc<tests::Hooks>,
}

impl AsyncSqlite {
    /// Finish any detached statement before closing the connection.
    /// Called by the pool on a blocking worker, or outside Tokio during teardown.
    pub(crate) fn close(self) {
        let connection = self.inner.clone().blocking_lock_owned();
        #[cfg(test)]
        self.hooks
            .wait("close", tests::Phase::Before)
            .expect("close test gate");
        drop(self);
        drop(connection);
    }

    pub fn new(inner: Connection, stale: Arc<AtomicBool>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(inner)),
            stale,
            #[cfg(test)]
            hooks: Arc::default(),
        }
    }

    async fn run<T, F>(
        &self,
        statement: Statement,
        allow_stale: bool,
        operation: F,
    ) -> Result<T, Error>
    where
        T: Send + 'static,
        F: FnOnce(&Connection, Statement) -> Result<T, Error> + Send + 'static,
    {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| Error::Internal("SQLite operations require a Tokio runtime".to_owned()))?;
        // Lock before dispatch: a cancelled caller cannot return its pooled
        // handle and let the next borrower overtake its already-issued SQL.
        let conn = self.inner.clone().lock_owned().await;
        if !allow_stale && self.stale.load(Ordering::SeqCst) {
            return Err(Error::Internal("Discarded SQLite connection".to_owned()));
        }
        let stale = self.stale.clone();
        #[cfg(test)]
        let hooks = self.hooks.clone();
        let span = tracing::Span::current();
        runtime
            .spawn_blocking(move || {
                span.in_scope(move || {
                    let _panic_guard = PanicGuard(stale);
                    #[cfg(test)]
                    let sql = statement.clone().to_sql()?.0;
                    #[cfg(test)]
                    hooks.wait(&sql, tests::Phase::Before)?;
                    let result = operation(&conn, statement);
                    #[cfg(test)]
                    hooks.wait(&sql, tests::Phase::After)?;
                    result
                })
            })
            .await
            .map_err(|error| Error::Database(Box::new(error)))?
    }

    fn run_stmt<T>(
        conn: &Connection,
        statement: Statement,
        operation: impl FnOnce(&mut CachedStatement<'_>) -> Result<T, SqliteError>,
    ) -> Result<T, Error> {
        let (sql, mut stmt) = Self::get_stmt(conn, statement)?;
        run_db_operation_sync(&sql, || operation(&mut stmt), to_sqlite_error)
    }

    fn get_stmt<'a>(
        conn: &'a Connection,
        statement: Statement,
    ) -> Result<(String, CachedStatement<'a>), Error> {
        let (sql, placeholder_values) = statement.to_sql()?;

        let new_sql = sql.trim().trim_end_matches("FOR UPDATE");

        let mut stmt = conn
            .prepare_cached(new_sql)
            .map_err(|e| Error::Database(Box::new(e)))?;

        for (i, value) in placeholder_values.into_iter().enumerate() {
            stmt.raw_bind_parameter(i + 1, to_sqlite(value))
                .map_err(|e| Error::Database(Box::new(e)))?;
        }

        Ok((sql, stmt))
    }
}

struct PanicGuard(Arc<AtomicBool>);

impl Drop for PanicGuard {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.0.store(true, Ordering::SeqCst);
        }
    }
}

#[inline(always)]
fn to_sqlite_error(err: SqliteError) -> Error {
    tracing::error!("Failed query with error {:?}", err);
    if let rusqlite::Error::SqliteFailure(
        ffi::Error {
            code,
            extended_code,
        },
        _,
    ) = err
    {
        if code == ErrorCode::ConstraintViolation
            && (extended_code == ffi::SQLITE_CONSTRAINT_PRIMARYKEY
                || extended_code == ffi::SQLITE_CONSTRAINT_UNIQUE)
        {
            Error::Duplicate
        } else {
            Error::Database(Box::new(err))
        }
    } else {
        Error::Database(Box::new(err))
    }
}

/// SQLite trasanction handler
#[allow(missing_debug_implementations)]
pub struct SQLiteTransactionHandler;

#[async_trait::async_trait]
impl DatabaseTransaction<AsyncSqlite> for SQLiteTransactionHandler {
    /// Consumes the current transaction committing the changes
    async fn commit(conn: &mut AsyncSqlite) -> Result<(), Error> {
        query("COMMIT")?.execute(conn).await?;
        Ok(())
    }

    /// Begin a transaction
    async fn begin(conn: &mut AsyncSqlite) -> Result<(), Error> {
        query("BEGIN IMMEDIATE")?.execute(conn).await?;
        Ok(())
    }

    /// Consumes the transaction rolling back all changes
    async fn rollback(conn: &mut AsyncSqlite) -> Result<(), Error> {
        let stale = conn.stale.clone();
        conn.run(query("ROLLBACK")?, true, move |conn, statement| {
            // A cancelled COMMIT may already have completed. Never issue a
            // later rollback against a different borrower's transaction.
            if conn.is_autocommit() {
                return Ok(());
            }
            let result =
                AsyncSqlite::run_stmt(conn, statement, |stmt| stmt.raw_execute()).map(|_| ());
            if result.is_err() || !conn.is_autocommit() {
                stale.store(true, Ordering::SeqCst);
            }
            result
        })
        .await
    }
}

impl DatabaseConnector for AsyncSqlite {
    type Transaction = SQLiteTransactionHandler;
}

#[async_trait::async_trait]
impl DatabaseExecutor for AsyncSqlite {
    fn name() -> &'static str {
        "sqlite"
    }

    async fn execute(&self, statement: Statement) -> Result<usize, Error> {
        self.run(statement, false, |conn, statement| {
            Self::run_stmt(conn, statement, |stmt| stmt.raw_execute())
        })
        .await
    }

    async fn fetch_one(&self, statement: Statement) -> Result<Option<Vec<Column>>, Error> {
        self.run(statement, false, |conn, statement| {
            Self::run_stmt(conn, statement, |stmt| {
                let columns = stmt.column_count();
                let mut rows = stmt.raw_query();
                rows.next()?
                    .map(|row| {
                        (0..columns)
                            .map(|i| row.get(i).map(from_sqlite))
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .transpose()
            })
        })
        .await
    }

    async fn fetch_all(&self, statement: Statement) -> Result<Vec<Vec<Column>>, Error> {
        self.run(statement, false, |conn, statement| {
            Self::run_stmt(conn, statement, |stmt| {
                let columns = stmt.column_count();
                let mut rows = stmt.raw_query();
                let mut results = vec![];
                while let Some(row) = rows.next()? {
                    results.push(
                        (0..columns)
                            .map(|i| row.get(i).map(from_sqlite))
                            .collect::<Result<Vec<_>, _>>()?,
                    );
                }
                Ok(results)
            })
        })
        .await
    }

    async fn pluck(&self, statement: Statement) -> Result<Option<Column>, Error> {
        self.run(statement, false, |conn, statement| {
            Self::run_stmt(conn, statement, |stmt| {
                let mut rows = stmt.raw_query();
                rows.next()?
                    .map(|row| row.get(0usize).map(from_sqlite))
                    .transpose()
            })
        })
        .await
    }

    async fn batch(&self, statement: Statement) -> Result<(), Error> {
        self.run(statement, false, |conn, mut statement| {
            let part = statement
                .parts
                .pop()
                .ok_or(Error::Internal("Empty SQL".to_owned()))?;
            if !statement.parts.is_empty() || matches!(part, SqlPart::Placeholder(_, _)) {
                return Err(Error::Internal(
                    "Invalid usage, batch does not support placeholders".to_owned(),
                ));
            }
            if let SqlPart::Raw(sql) = part {
                run_db_operation_sync(&sql, || conn.execute_batch(&sql), to_sqlite_error)
            } else {
                Err(Error::Internal("Expected raw SQL batch".to_owned()))
            }
        })
        .await
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod connection_contract_tests;

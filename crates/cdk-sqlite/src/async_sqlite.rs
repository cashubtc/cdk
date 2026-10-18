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
    inner: Option<Arc<Mutex<Connection>>>,
    stale: Arc<AtomicBool>,
    #[cfg(test)]
    hooks: Arc<tests::Hooks>,
}

impl AsyncSqlite {
    pub fn new(inner: Connection) -> Self {
        Self::with_stale(inner, Arc::new(AtomicBool::new(false)))
    }

    pub(crate) fn with_stale(inner: Connection, stale: Arc<AtomicBool>) -> Self {
        Self {
            inner: Some(Arc::new(Mutex::new(inner))),
            stale,
            #[cfg(test)]
            hooks: Arc::default(),
        }
    }

    async fn run<T, F>(&self, statement: Statement, operation: F) -> Result<T, Error>
    where
        T: Send + 'static,
        F: FnOnce(&Connection, Statement) -> Result<T, Error> + Send + 'static,
    {
        self.run_inner(statement, false, operation).await
    }

    async fn run_inner<T, F>(
        &self,
        statement: Statement,
        allow_stale: bool,
        operation: F,
    ) -> Result<T, Error>
    where
        T: Send + 'static,
        F: FnOnce(&Connection, Statement) -> Result<T, Error> + Send + 'static,
    {
        let inner = self
            .inner
            .as_ref()
            .ok_or(Error::Internal("Missing SQLite connection".to_owned()))?;
        // Lock before dispatch: a cancelled caller cannot return its pooled
        // handle and let the next borrower overtake its already-issued SQL.
        let conn = inner.clone().lock_owned().await;
        if !allow_stale && self.stale.load(Ordering::SeqCst) {
            return Err(Error::Internal("Discarded SQLite connection".to_owned()));
        }
        let stale = self.stale.clone();
        #[cfg(test)]
        let hooks = self.hooks.clone();
        tokio::task::spawn_blocking(move || {
            let _panic_guard = PanicGuard(stale);
            #[cfg(test)]
            let sql = {
                let mut copy = Statement::default();
                copy.parts = statement.parts.clone();
                copy.to_sql()?.0
            };
            #[cfg(test)]
            hooks.wait(&sql, tests::Phase::Before)?;
            let result = operation(&conn, statement);
            #[cfg(test)]
            hooks.wait(&sql, tests::Phase::After)?;
            result
        })
        .await
        .map_err(|error| Error::Database(Box::new(error)))?
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

impl Drop for AsyncSqlite {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take() {
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                // Closing the last WAL connection can perform synchronous I/O.
                runtime.spawn_blocking(move || drop(inner));
            }
        }
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
        conn.run_inner(query("ROLLBACK")?, true, move |conn, statement| {
            // A cancelled COMMIT may already have completed. Never issue a
            // later rollback against a different borrower's transaction.
            if conn.is_autocommit() {
                return Ok(());
            }
            let result = (|| {
                let (sql, mut stmt) = AsyncSqlite::get_stmt(conn, statement)?;
                run_db_operation_sync(&sql, || stmt.raw_execute(), to_sqlite_error)?;
                Ok(())
            })();
            if result.is_err() || !conn.is_autocommit() {
                stale.store(true, Ordering::SeqCst);
            }
            result
        })
        .await
    }
}

impl DatabaseConnector for AsyncSqlite {
    fn mark_unusable(&mut self) {
        self.stale.store(true, Ordering::SeqCst);
    }

    type Transaction = SQLiteTransactionHandler;
}

#[async_trait::async_trait]
impl DatabaseExecutor for AsyncSqlite {
    fn name() -> &'static str {
        "sqlite"
    }

    async fn execute(&self, statement: Statement) -> Result<usize, Error> {
        self.run(statement, |conn, statement| {
            let (sql, mut stmt) = Self::get_stmt(conn, statement)?;
            run_db_operation_sync(&sql, || stmt.raw_execute(), to_sqlite_error)
        })
        .await
    }

    async fn fetch_one(&self, statement: Statement) -> Result<Option<Vec<Column>>, Error> {
        self.run(statement, |conn, statement| {
            let (sql, mut stmt) = Self::get_stmt(conn, statement)?;
            run_db_operation_sync(
                &sql,
                || {
                    let columns = stmt.column_count();
                    let mut rows = stmt.raw_query();
                    rows.next()?
                        .map(|row| {
                            (0..columns)
                                .map(|i| row.get(i).map(from_sqlite))
                                .collect::<Result<Vec<_>, _>>()
                        })
                        .transpose()
                },
                to_sqlite_error,
            )
        })
        .await
    }

    async fn fetch_all(&self, statement: Statement) -> Result<Vec<Vec<Column>>, Error> {
        self.run(statement, |conn, statement| {
            let (sql, mut stmt) = Self::get_stmt(conn, statement)?;
            let columns = stmt.column_count();
            run_db_operation_sync(
                &sql,
                || {
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
                },
                to_sqlite_error,
            )
        })
        .await
    }

    async fn pluck(&self, statement: Statement) -> Result<Option<Column>, Error> {
        self.run(statement, |conn, statement| {
            let (sql, mut stmt) = Self::get_stmt(conn, statement)?;
            run_db_operation_sync(
                &sql,
                || {
                    let mut rows = stmt.raw_query();
                    rows.next()?
                        .map(|row| row.get(0usize).map(from_sqlite))
                        .transpose()
                },
                to_sqlite_error,
            )
        })
        .await
    }

    async fn batch(&self, statement: Statement) -> Result<(), Error> {
        self.run(statement, |conn, mut statement| {
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
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{mpsc, Arc, Mutex};
    use std::time::Duration;

    use cdk_common::database::Error;
    use cdk_sql_common::database::ConnectionWithTransaction;
    use cdk_sql_common::pool::{self, DatabaseConfig, DatabasePool, Pool, PooledResource};
    use cdk_sql_common::stmt::query;
    use tokio::sync::oneshot;

    use super::AsyncSqlite;
    use crate::common::{Config, SqliteConnectionManager};

    mod cancellation;
    mod contention;
    mod creation;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("cdk-contention-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) enum Phase {
        Before,
        After,
    }

    #[derive(Debug)]
    enum Release {
        Continue,
        Fail,
        Panic,
    }

    #[derive(Debug)]
    struct Gate {
        sql: &'static str,
        phase: Phase,
        entered: oneshot::Sender<()>,
        release: mpsc::Receiver<Release>,
    }

    #[derive(Debug, Default)]
    pub(super) struct Hooks(Mutex<Vec<Gate>>);

    impl Hooks {
        fn arm(&self, sql: &'static str, phase: Phase) -> Barrier {
            let (entered, observed) = oneshot::channel();
            let (release, waiting) = mpsc::channel();
            self.0.lock().unwrap().push(Gate {
                sql,
                phase,
                entered,
                release: waiting,
            });
            Barrier {
                observed: Some(observed),
                release: Some(release),
            }
        }

        pub(super) fn wait(&self, sql: &str, phase: Phase) -> Result<(), Error> {
            let gate = {
                let mut gates = self.0.lock().unwrap();
                gates
                    .iter()
                    .position(|gate| gate.sql == sql.trim() && gate.phase == phase)
                    .map(|index| gates.remove(index))
            };
            if let Some(gate) = gate {
                let _ = gate.entered.send(());
                match gate.release.recv_timeout(Duration::from_secs(5)).unwrap() {
                    Release::Continue => {}
                    Release::Fail => {
                        return Err(Error::Internal("injected driver failure".to_owned()))
                    }
                    Release::Panic => panic!("injected blocking worker panic"),
                }
            }
            Ok(())
        }
    }

    struct Barrier {
        observed: Option<oneshot::Receiver<()>>,
        release: Option<mpsc::Sender<Release>>,
    }

    impl Barrier {
        async fn entered(&mut self) {
            tokio::time::timeout(Duration::from_secs(2), self.observed.take().unwrap())
                .await
                .unwrap()
                .unwrap();
        }

        fn release(mut self, action: Release) {
            self.release.take().unwrap().send(action).unwrap();
        }
    }

    impl Drop for Barrier {
        fn drop(&mut self) {
            if let Some(release) = self.release.take() {
                let _ = release.send(Release::Continue);
            }
        }
    }

    #[derive(Debug, Clone)]
    struct TestConfig {
        path: PathBuf,
        creates: Arc<AtomicUsize>,
        constructor: Arc<Hooks>,
        statements: Arc<Hooks>,
    }

    impl DatabaseConfig for TestConfig {
        fn max_size(&self) -> usize {
            1
        }
        fn default_timeout(&self) -> Duration {
            Duration::from_secs(2)
        }
    }

    #[derive(Debug)]
    struct TestManager;

    impl DatabasePool for TestManager {
        type Config = TestConfig;
        type Connection = AsyncSqlite;
        type Error = rusqlite::Error;

        fn new_resource(
            config: &TestConfig,
            stale: Arc<AtomicBool>,
            timeout: Duration,
        ) -> Result<AsyncSqlite, pool::Error<Self::Error>> {
            config.creates.fetch_add(1, Ordering::SeqCst);
            config
                .constructor
                .wait("open", Phase::Before)
                .map_err(|_| pool::Error::Poison)?;
            let mut connection =
                SqliteConnectionManager::new_resource(&Config::from(&config.path), stale, timeout)?;
            connection.hooks = config.statements.clone();
            Ok(connection)
        }
    }

    type Transaction = ConnectionWithTransaction<AsyncSqlite, PooledResource<TestManager>>;

    struct Fixture {
        pool: Arc<Pool<TestManager>>,
        config: TestConfig,
        _directory: TestDirectory,
    }

    impl Fixture {
        fn unopened() -> Self {
            let directory = TestDirectory::new();
            let config = TestConfig {
                path: directory.0.join("wallet.sqlite"),
                creates: Arc::default(),
                constructor: Arc::default(),
                statements: Arc::default(),
            };
            Self {
                pool: Pool::new(config.clone()),
                config,
                _directory: directory,
            }
        }

        async fn new() -> Self {
            let fixture = Self::unopened();
            let connection = fixture.pool.get().await.unwrap();
            query("CREATE TABLE data (id INTEGER PRIMARY KEY)")
                .unwrap()
                .execute(&*connection)
                .await
                .unwrap();
            drop(connection);
            fixture
        }

        async fn assert_loan_held(&self) {
            let result = self.pool.get_timeout(Duration::from_millis(20)).await;
            assert!(matches!(result, Err(pool::Error::Timeout)), "{result:?}");
            assert_eq!(self.config.creates.load(Ordering::SeqCst), 1);
        }

        async fn transaction(&self) -> Transaction {
            Transaction::new(self.pool.get().await.unwrap())
                .await
                .unwrap()
        }

        async fn assert_clean_and_write(&self, expected: Vec<i64>) {
            let transaction = self.transaction().await;
            let rows = query("SELECT id FROM data ORDER BY id")
                .unwrap()
                .fetch_all(&transaction)
                .await
                .unwrap();
            let values: Vec<_> = rows
                .into_iter()
                .map(|row| match &row[0] {
                    cdk_sql_common::value::Value::Integer(value) => *value,
                    other => panic!("unexpected column {other:?}"),
                })
                .collect();
            assert_eq!(values, expected);
            query("INSERT INTO data VALUES (99)")
                .unwrap()
                .execute(&transaction)
                .await
                .unwrap();
            transaction.commit().await.unwrap();
            // There must be no late orphan cleanup capable of rolling back this
            // borrower's independently committed sentinel.
            tokio::task::yield_now().await;
            let connection = self.pool.get().await.unwrap();
            let rows = query("SELECT id FROM data WHERE id = 99")
                .unwrap()
                .fetch_all(&*connection)
                .await
                .unwrap();
            assert_eq!(rows.len(), 1);
        }
    }
}

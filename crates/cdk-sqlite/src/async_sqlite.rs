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
    mod close;
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
    pub(super) struct Hooks(Mutex<Option<Gate>>);

    impl Hooks {
        fn arm(&self, sql: &'static str, phase: Phase) -> Barrier {
            let (entered, observed) = oneshot::channel();
            let (release, waiting) = mpsc::channel();
            let mut gate = self.0.lock().unwrap();
            assert!(gate.is_none(), "a test gate is already armed");
            *gate = Some(Gate {
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
                let mut gate = self.0.lock().unwrap();
                match gate.as_ref() {
                    Some(armed) if armed.sql == sql.trim() && armed.phase == phase => gate.take(),
                    _ => None,
                }
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

        fn drop(resource: Self::Connection) {
            SqliteConnectionManager::drop(resource);
        }

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

#[cfg(test)]
mod connection_contract_tests {
    use std::fs::remove_file;
    use std::sync::mpsc::channel;
    use std::thread;
    use std::time::{Duration, Instant};

    use cdk_sql_common::database::ConnectionWithTransaction;
    use cdk_sql_common::pool::Pool;
    use tokio::runtime::Builder;
    use tokio::task::JoinSet;
    use tokio::time::sleep;

    use super::*;
    use crate::common::{Config, SqliteConnectionManager};

    fn in_memory() -> AsyncSqlite {
        AsyncSqlite::new(
            Connection::open_in_memory().expect("in memory database"),
            Arc::new(AtomicBool::new(false)),
        )
    }

    fn temp_path(prefix: &str) -> String {
        std::env::temp_dir()
            .join(format!("cdk-{prefix}-{}.sqlite", uuid::Uuid::new_v4()))
            .to_str()
            .expect("utf8 path")
            .to_owned()
    }

    fn remove_database(path: &str) {
        let _ = remove_file(path);
        let _ = remove_file(format!("{path}-wal"));
        let _ = remove_file(format!("{path}-shm"));
    }

    fn config_for(path: &str) -> Config {
        #[cfg(not(feature = "sqlcipher"))]
        let config: Config = path.into();
        #[cfg(feature = "sqlcipher")]
        let config: Config = (path, "test").into();

        config
    }

    /// Runs `writers` write transactions at once, each keeping the write lock for `statements`
    /// inserts plus an optional suspension, and reports what every writer got.
    async fn write_burst(
        writers: usize,
        statements: usize,
        hold: Option<Duration>,
    ) -> Vec<Result<(), Error>> {
        let path = temp_path("write-burst");
        let pool = Pool::<SqliteConnectionManager>::new(config_for(&path));

        {
            let conn = pool.get().await.expect("connection");
            query("CREATE TABLE t (v INTEGER)")
                .expect("sql")
                .execute(&*conn)
                .await
                .expect("create table");
        }

        let mut burst = JoinSet::new();
        for _ in 0..writers {
            let pool = pool.clone();
            burst.spawn(async move {
                let conn = pool
                    .get()
                    .await
                    .map_err(|err| Error::Internal(err.to_string()))?;
                let tx = ConnectionWithTransaction::new(conn).await?;

                if let Some(hold) = hold {
                    sleep(hold).await;
                }

                for v in 0..statements {
                    query("INSERT INTO t (v) VALUES (:v)")?
                        .bind("v", i64::try_from(v).expect("fits in i64"))
                        .execute(&tx)
                        .await?;
                }

                tx.commit().await
            });
        }

        let mut results = Vec::with_capacity(writers);
        while let Some(joined) = burst.join_next().await {
            results.push(joined.expect("writer task"));
        }

        drop(pool);
        remove_database(&path);

        results
    }

    fn assert_burst_is_clean(results: &[Result<(), Error>], elapsed: Duration) {
        assert!(
            results.iter().all(|result| result.is_ok()),
            "writers failed: {results:?}"
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "burst took {elapsed:?}, the write lock was not released promptly"
        );
    }

    /// Writers queueing for the write lock must not stop its holder from committing, here with a
    /// transaction long enough that the runtime suspends it part way through.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_writers_do_not_starve_a_long_transaction() {
        let start = Instant::now();
        let results = write_burst(16, 130, None).await;

        assert_burst_is_clean(&results, start.elapsed());
    }

    /// The same guarantee when the holder is suspended by something other than the length of its
    /// transaction, with fewer runtime workers than writers.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_writers_do_not_starve_a_suspended_transaction() {
        let start = Instant::now();
        let results = write_burst(4, 4, Some(Duration::from_millis(1))).await;

        assert_burst_is_clean(&results, start.elapsed());
    }

    #[tokio::test]
    async fn rollback_discards_the_transaction_and_is_a_noop_without_begin() {
        let mut db = in_memory();

        SQLiteTransactionHandler::rollback(&mut db)
            .await
            .expect("rollback outside a transaction");

        db.batch(query("CREATE TABLE t (v INTEGER)").expect("sql"))
            .await
            .expect("create table");

        SQLiteTransactionHandler::begin(&mut db)
            .await
            .expect("begin");
        db.execute(query("INSERT INTO t (v) VALUES (42)").expect("sql"))
            .await
            .expect("insert");
        SQLiteTransactionHandler::rollback(&mut db)
            .await
            .expect("rollback");

        assert_eq!(
            db.pluck(query("SELECT v FROM t").expect("sql"))
                .await
                .expect("select"),
            None
        );
    }

    /// Explicit close finishes checkpointing off async workers before a file copy.
    #[test]
    fn closing_pool_completes_checkpoint_before_file_backup() {
        let path = temp_path("close-before-backup");
        let backup = temp_path("closed-backup");
        let runtime = Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();

        let values = runtime.block_on(async {
            let pool = Pool::<SqliteConnectionManager>::new(config_for(&path));
            let connection = pool.get().await.unwrap();
            query("CREATE TABLE data (id INTEGER)")
                .unwrap()
                .execute(&*connection)
                .await
                .unwrap();
            query("PRAGMA wal_checkpoint(TRUNCATE)")
                .unwrap()
                .fetch_all(&*connection)
                .await
                .unwrap();
            query("INSERT INTO data VALUES (42)")
                .unwrap()
                .execute(&*connection)
                .await
                .unwrap();
            drop(connection);

            // Closing must yield even when the blocking worker is occupied.
            let (release, wait) = channel();
            let (started, entered) = tokio::sync::oneshot::channel();
            let worker = tokio::task::spawn_blocking(move || {
                started.send(()).unwrap();
                wait.recv_timeout(Duration::from_secs(10)).unwrap();
            });
            entered.await.unwrap();
            let mut closing = Box::pin(pool.close());
            assert!(
                tokio::time::timeout(Duration::from_millis(20), &mut closing)
                    .await
                    .is_err()
            );
            release.send(()).unwrap();
            worker.await.unwrap();
            closing.await.unwrap();
            // Retain the pool handle: close must work independently of Arc counts.
            assert!(matches!(
                pool.get().await,
                Err(cdk_sql_common::pool::Error::Closed)
            ));
            std::fs::copy(&path, &backup).unwrap();

            let connection = Connection::open(&backup).unwrap();
            #[cfg(feature = "sqlcipher")]
            connection.pragma_update(None, "key", "test").unwrap();
            let values: Vec<i64> = connection
                .prepare("SELECT id FROM data")
                .unwrap()
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            values
        });

        drop(runtime);
        remove_database(&path);
        remove_database(&backup);
        assert_eq!(values, vec![42]);
    }

    /// A connection outliving its runtime must not hold up that runtime's shutdown.
    #[test]
    fn dropping_a_runtime_does_not_hang_on_a_live_connection() {
        let (done, is_done) = channel();

        thread::spawn(move || {
            let runtime = Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            runtime.block_on(async {
                let db: &'static AsyncSqlite = Box::leak(Box::new(in_memory()));

                db.pluck(query("SELECT 1").expect("sql"))
                    .await
                    .expect("select");
            });
            drop(runtime);

            done.send(()).expect("report the shutdown");
        });

        is_done
            .recv_timeout(Duration::from_secs(10))
            .expect("the runtime shut down while a connection was still alive");
    }
}

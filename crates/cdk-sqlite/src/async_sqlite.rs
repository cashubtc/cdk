//! Simple SQLite
use std::convert::identity;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use cdk_common::database::Error;
use cdk_sql_common::database::{DatabaseConnector, DatabaseExecutor, DatabaseTransaction};
use cdk_sql_common::run_db_operation;
use cdk_sql_common::stmt::{query, Column, SqlPart, Statement};
use cdk_sql_common::value::Value;
use rusqlite::{ffi, CachedStatement, Connection, Error as SqliteError, ErrorCode};
use tokio::runtime::Handle;
use tokio::sync::Mutex;

use crate::common::{from_sqlite, to_sqlite};

/// Async Sqlite wrapper
#[derive(Debug)]
pub struct AsyncSqlite {
    conn: Arc<Mutex<Connection>>,
    stale: Arc<AtomicBool>,
}

impl AsyncSqlite {
    /// Shares the connection behind an async mutex so each statement can run on a blocking thread,
    /// since every rusqlite call blocks.
    ///
    /// The blocking work is one task per statement rather than a parked worker loop, because
    /// dropping a runtime waits for every blocking task it owns and a loop parked until its last
    /// handle drops would never return.
    pub fn new(conn: Connection, stale: Arc<AtomicBool>) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
            stale,
        }
    }

    /// Runs `f` on a blocking thread holding the connection, timed as `info`, marking the
    /// connection stale when `f` panics and leaves sqlite's state ambiguous.
    ///
    /// The guard is taken before the task is spawned because tokio's mutex is FIFO, which is what
    /// keeps a rollback queued behind the cancelled statement it has to undo.
    async fn call<T, F>(&self, info: &str, f: F) -> Result<T, Error>
    where
        F: FnOnce(&Connection) -> Result<T, Error> + Send + 'static,
        T: Send + 'static,
    {
        run_db_operation(
            info,
            async {
                let handle = Handle::try_current()
                    .map_err(|_| Error::Internal("sqlite calls need a tokio runtime".to_owned()))?;
                let guard = Arc::clone(&self.conn).lock_owned().await;

                handle
                    .spawn_blocking(move || f(&guard))
                    .await
                    .map_err(|err| {
                        if err.is_panic() {
                            self.stale.store(true, Ordering::SeqCst);
                            Error::Internal("sqlite call panicked".to_owned())
                        } else {
                            Error::Internal("sqlite call was cancelled".to_owned())
                        }
                    })?
            },
            identity,
        )
        .await
    }
}

/// `FOR UPDATE` is stripped because SQLite has no row locks and the shared SQL is written for
/// Postgres.
fn prepare<'a>(
    conn: &'a Connection,
    sql: &str,
    placeholder_values: Vec<Value>,
) -> Result<CachedStatement<'a>, Error> {
    let mut stmt = conn
        .prepare_cached(sql.trim().trim_end_matches("FOR UPDATE"))
        .map_err(|e| Error::Database(Box::new(e)))?;

    for (i, value) in placeholder_values.into_iter().enumerate() {
        stmt.raw_bind_parameter(i + 1, to_sqlite(value))
            .map_err(|e| Error::Database(Box::new(e)))?;
    }

    Ok(stmt)
}

fn first_row(stmt: &mut CachedStatement<'_>) -> Result<Option<Vec<Column>>, SqliteError> {
    let columns = stmt.column_count();
    let mut rows = stmt.raw_query();

    rows.next()?
        .map(|row| {
            (0..columns)
                .map(|i| row.get(i).map(from_sqlite))
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()
}

fn all_rows(stmt: &mut CachedStatement<'_>) -> Result<Vec<Vec<Column>>, SqliteError> {
    let columns = stmt.column_count();
    let mut rows = stmt.raw_query();
    let mut results = vec![];

    while let Some(row) = rows.next()? {
        results.push(
            (0..columns)
                .map(|i| row.get(i).map(from_sqlite))
                .collect::<Result<Vec<_>, _>>()?,
        )
    }

    Ok(results)
}

fn first_column(stmt: &mut CachedStatement<'_>) -> Result<Option<Column>, SqliteError> {
    let mut rows = stmt.raw_query();

    rows.next()?
        .map(|row| row.get(0usize).map(from_sqlite))
        .transpose()
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

    /// Consumes the transaction rolling back all changes, skipping the ROLLBACK when nothing was
    /// begun, since the resulting error would discard the connection and with it an in-memory
    /// database.
    async fn rollback(conn: &mut AsyncSqlite) -> Result<(), Error> {
        if conn
            .call("is_autocommit", |inner: &Connection| {
                Ok(inner.is_autocommit())
            })
            .await?
        {
            return Ok(());
        }
        query("ROLLBACK")?.execute(conn).await?;
        Ok(())
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
        let (sql, placeholder_values) = statement.to_sql()?;
        let prepared: Arc<str> = sql.as_str().into();

        self.call(&sql, move |conn| {
            prepare(conn, &prepared, placeholder_values)?
                .raw_execute()
                .map_err(to_sqlite_error)
        })
        .await
    }

    async fn fetch_one(&self, statement: Statement) -> Result<Option<Vec<Column>>, Error> {
        let (sql, placeholder_values) = statement.to_sql()?;
        let prepared: Arc<str> = sql.as_str().into();

        self.call(&sql, move |conn| {
            first_row(&mut prepare(conn, &prepared, placeholder_values)?).map_err(to_sqlite_error)
        })
        .await
    }

    async fn fetch_all(&self, statement: Statement) -> Result<Vec<Vec<Column>>, Error> {
        let (sql, placeholder_values) = statement.to_sql()?;
        let prepared: Arc<str> = sql.as_str().into();

        self.call(&sql, move |conn| {
            all_rows(&mut prepare(conn, &prepared, placeholder_values)?).map_err(to_sqlite_error)
        })
        .await
    }

    async fn pluck(&self, statement: Statement) -> Result<Option<Column>, Error> {
        let (sql, placeholder_values) = statement.to_sql()?;
        let prepared: Arc<str> = sql.as_str().into();

        self.call(&sql, move |conn| {
            first_column(&mut prepare(conn, &prepared, placeholder_values)?)
                .map_err(to_sqlite_error)
        })
        .await
    }

    async fn batch(&self, mut statement: Statement) -> Result<(), Error> {
        let sql = {
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
                sql
            } else {
                unreachable!()
            }
        };
        let batch = Arc::clone(&sql);

        self.call(&sql, move |conn| {
            conn.execute_batch(&batch).map_err(to_sqlite_error)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use std::fs::remove_file;
    use std::sync::mpsc::channel;
    use std::thread;
    use std::time::{Duration, Instant};

    use cdk_sql_common::database::ConnectionWithTransaction;
    use cdk_sql_common::pool::Pool;
    use cdk_sql_common::value::Value;
    use tokio::runtime::Builder;
    use tokio::task::JoinSet;
    use tokio::time::{sleep, timeout};

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
    async fn transaction_round_trip_sees_its_own_writes() {
        let mut db = in_memory();

        db.batch(query("CREATE TABLE t (v INTEGER)").expect("sql"))
            .await
            .expect("create table");

        SQLiteTransactionHandler::begin(&mut db)
            .await
            .expect("begin");
        db.execute(query("INSERT INTO t (v) VALUES (42)").expect("sql"))
            .await
            .expect("insert");
        SQLiteTransactionHandler::commit(&mut db)
            .await
            .expect("commit");

        assert_eq!(
            db.pluck(query("SELECT v FROM t").expect("sql"))
                .await
                .expect("select"),
            Some(Value::Integer(42))
        );
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

    #[tokio::test]
    async fn a_panicking_call_marks_the_connection_stale() {
        let stale = Arc::new(AtomicBool::new(false));
        let db = AsyncSqlite::new(
            Connection::open_in_memory().expect("in memory database"),
            stale.clone(),
        );

        assert!(matches!(
            db.call("panic", |_: &Connection| -> Result<(), Error> {
                panic!("the call dies here")
            })
            .await,
            Err(Error::Internal(_))
        ));

        assert!(
            stale.load(Ordering::SeqCst),
            "a panicking call left the connection usable"
        );
    }

    /// A panicking call must take its connection out of the pool rather than leave one with
    /// ambiguous sqlite state to be handed out again, asserted against an in-memory database
    /// because discarding its only connection is the one case the pool refuses to replace.
    #[tokio::test]
    async fn a_panicking_call_takes_its_connection_out_of_the_pool() {
        let pool = Pool::<SqliteConnectionManager>::new(config_for(":memory:"));

        {
            let conn = pool.get().await.expect("connection");
            assert!(matches!(
                conn.call("panic", |_: &Connection| -> Result<(), Error> {
                    panic!("the call dies here")
                })
                .await,
                Err(Error::Internal(_))
            ));
        }

        assert!(
            pool.get().await.is_err(),
            "the panicked connection was handed out again"
        );
    }

    /// A cancelled statement keeps running to completion, so the next statement must queue behind
    /// it instead of overtaking it, which is what stops a rollback preceding the write it undoes.
    #[tokio::test]
    async fn a_cancelled_statement_runs_before_the_next_one() {
        let db = in_memory();

        db.batch(query("CREATE TABLE t (v INTEGER)").expect("sql"))
            .await
            .expect("create table");

        let slow = db.execute(
            query(
                r#"
                INSERT INTO t (v)
                SELECT count(*) FROM (
                    WITH RECURSIVE c(x) AS (
                        SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 1000000
                    )
                    SELECT x FROM c
                )
                "#,
            )
            .expect("sql"),
        );

        assert!(
            timeout(Duration::from_millis(1), slow).await.is_err(),
            "the statement finished before it could be cancelled"
        );

        assert_eq!(
            db.pluck(query("SELECT count(*) FROM t").expect("sql"))
                .await
                .expect("select"),
            Some(Value::Integer(1))
        );
    }

    /// A connection outliving its runtime must not hold up that runtime's shutdown, which is why
    /// the blocking work is one task per statement and never a parked loop.
    #[test]
    fn dropping_a_runtime_does_not_hang_on_a_live_connection() {
        let (done, is_done) = channel();

        thread::spawn(move || {
            let runtime = Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            let db: &'static AsyncSqlite = Box::leak(Box::new(in_memory()));

            runtime.block_on(async {
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

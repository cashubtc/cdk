//! Simple SQLite
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use cdk_common::database::Error;
use cdk_sql_common::database::{DatabaseConnector, DatabaseExecutor, DatabaseTransaction};
use cdk_sql_common::stmt::{query, Column, SqlPart, Statement};
use cdk_sql_common::value::Value;
use cdk_sql_common::SLOW_QUERY_THRESHOLD_MS;
use rusqlite::{ffi, CachedStatement, Connection, Error as SqliteError, ErrorCode, Row};
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot};
use tokio::task::spawn_blocking;

use crate::common::{from_sqlite, to_sqlite};

/// Where a caller waits for its answer.
type Reply<T> = oneshot::Sender<Result<T, Error>>;

/// A statement for the task owning the connection, carrying the channel its answer goes back on.
enum Request {
    /// Runs the statement for its affected row count.
    Execute {
        sql: Arc<str>,
        values: Vec<Value>,
        reply: Reply<usize>,
    },
    /// Runs the statement for its first row.
    FetchOne {
        sql: Arc<str>,
        values: Vec<Value>,
        reply: Reply<Option<Vec<Column>>>,
    },
    /// Runs the statement for every row.
    FetchAll {
        sql: Arc<str>,
        values: Vec<Value>,
        reply: Reply<Vec<Vec<Column>>>,
    },
    /// Runs the statement for the first column of its first row.
    Pluck {
        sql: Arc<str>,
        values: Vec<Value>,
        reply: Reply<Option<Column>>,
    },
    /// Runs several statements with no placeholders.
    Batch { sql: Arc<str>, reply: Reply<()> },
    /// Asks whether the connection is outside a transaction.
    IsAutocommit { reply: Reply<bool> },
    /// Kills the connection, so the tests can reach the staleness path. The reply channel rides
    /// along unread because the unwind dropping it is what its caller observes.
    #[cfg(test)]
    Panic { _reply: Reply<()> },
}

/// Async Sqlite wrapper
#[derive(Debug)]
pub struct AsyncSqlite {
    sender: mpsc::UnboundedSender<Request>,
    stale: Arc<AtomicBool>,
}

impl AsyncSqlite {
    /// Hands the connection to a tokio task fed by a channel, so callers queue requests instead of
    /// sharing the connection, and the queue order is what keeps a rollback behind the statement
    /// it has to undo even when that statement's caller was cancelled.
    ///
    /// The task is an async one and each request gets its own blocking task, so an idle connection
    /// holds no thread and nothing of ours is parked when the runtime shuts down.
    pub fn new(conn: Connection, stale: Arc<AtomicBool>) -> Result<Self, SqliteError> {
        let Ok(handle) = Handle::try_current() else {
            return Err(SqliteError::SqliteFailure(
                ffi::Error::new(ffi::SQLITE_MISUSE),
                Some("sqlite connections must be opened inside a tokio runtime".to_owned()),
            ));
        };
        let (sender, mut receiver) = mpsc::unbounded_channel::<Request>();

        handle.spawn(async move {
            let mut conn = conn;

            while let Some(request) = receiver.recv().await {
                conn = match spawn_blocking(move || {
                    serve(&conn, request);
                    conn
                })
                .await
                {
                    Ok(conn) => conn,
                    Err(err) => {
                        tracing::error!("A sqlite request failed to return its connection: {err}");
                        return;
                    }
                };
            }
        });

        Ok(Self { sender, stale })
    }

    /// Takes the connection out of the pool, since a request that never answered leaves sqlite's
    /// state unknown and the connection is gone with the task that owned it.
    fn lost<T>(&self, reason: &str) -> Result<T, Error> {
        tracing::error!("{reason}");
        self.stale.store(true, Ordering::SeqCst);
        Err(Error::Internal(reason.to_owned()))
    }

    /// Kills the connection by panicking on its task.
    #[cfg(test)]
    async fn provoke_panic(&self) -> Result<(), Error> {
        let (reply, response) = oneshot::channel();
        let request = Request::Panic { _reply: reply };

        if self.sender.send(request).is_err() {
            return self.lost("the sqlite connection task is gone");
        }

        match response.await {
            Ok(result) => result,
            Err(_) => self.lost("the sqlite request did not answer"),
        }
    }
}

/// Runs one request against the connection, timing the statement alone rather than the wait that
/// preceded it.
fn serve(conn: &Connection, request: Request) {
    let info = match &request {
        Request::Execute { sql, .. }
        | Request::FetchOne { sql, .. }
        | Request::FetchAll { sql, .. }
        | Request::Pluck { sql, .. }
        | Request::Batch { sql, .. } => Arc::clone(sql),
        Request::IsAutocommit { .. } => "is_autocommit".into(),
        #[cfg(test)]
        Request::Panic { .. } => "panic".into(),
    };
    let start = Instant::now();

    tracing::trace!("Running db operation {info}");

    match request {
        Request::Execute { sql, values, reply } => {
            let result = match prepare(conn, &sql, values) {
                Ok(mut stmt) => stmt.raw_execute().map_err(to_sqlite_error),
                Err(err) => Err(err),
            };

            if reply.send(result).is_err() {
                tracing::debug!("Discarded a sqlite result, its caller is gone");
            }
        }
        Request::FetchOne { sql, values, reply } => {
            let result = match prepare(conn, &sql, values) {
                Err(err) => Err(err),
                Ok(mut stmt) => {
                    let count = stmt.column_count();
                    let mut rows = stmt.raw_query();

                    match rows.next() {
                        Ok(Some(row)) => row_columns(row, count).map(Some),
                        Ok(None) => Ok(None),
                        Err(err) => Err(to_sqlite_error(err)),
                    }
                }
            };

            if reply.send(result).is_err() {
                tracing::debug!("Discarded a sqlite result, its caller is gone");
            }
        }
        Request::FetchAll { sql, values, reply } => {
            if reply.send(all_rows(conn, &sql, values)).is_err() {
                tracing::debug!("Discarded a sqlite result, its caller is gone");
            }
        }
        Request::Pluck { sql, values, reply } => {
            let result = match prepare(conn, &sql, values) {
                Err(err) => Err(err),
                Ok(mut stmt) => {
                    let mut rows = stmt.raw_query();

                    match rows.next() {
                        Ok(Some(row)) => row
                            .get(0usize)
                            .map(from_sqlite)
                            .map(Some)
                            .map_err(to_sqlite_error),
                        Ok(None) => Ok(None),
                        Err(err) => Err(to_sqlite_error(err)),
                    }
                }
            };

            if reply.send(result).is_err() {
                tracing::debug!("Discarded a sqlite result, its caller is gone");
            }
        }
        Request::Batch { sql, reply } => {
            if reply
                .send(conn.execute_batch(&sql).map_err(to_sqlite_error))
                .is_err()
            {
                tracing::debug!("Discarded a sqlite result, its caller is gone");
            }
        }
        Request::IsAutocommit { reply } => {
            if reply.send(Ok(conn.is_autocommit())).is_err() {
                tracing::debug!("Discarded a sqlite result, its caller is gone");
            }
        }
        #[cfg(test)]
        Request::Panic { .. } => panic!("the request dies here"),
    }

    let elapsed = start.elapsed().as_millis();
    if elapsed > SLOW_QUERY_THRESHOLD_MS {
        tracing::warn!("[SLOW QUERY] Took {elapsed} ms: {info}");
    }
}

/// `FOR UPDATE` is stripped because SQLite has no row locks and the shared SQL is written for
/// Postgres.
fn prepare<'a>(
    conn: &'a Connection,
    sql: &str,
    values: Vec<Value>,
) -> Result<CachedStatement<'a>, Error> {
    let mut stmt = conn
        .prepare_cached(sql.trim().trim_end_matches("FOR UPDATE"))
        .map_err(prepare_error)?;

    for (i, value) in values.into_iter().enumerate() {
        stmt.raw_bind_parameter(i + 1, to_sqlite(value))
            .map_err(prepare_error)?;
    }

    Ok(stmt)
}

/// Keeps prepare and bind failures out of `to_sqlite_error`'s `Error::Duplicate` mapping, which
/// only makes sense for a statement that ran.
fn prepare_error(err: SqliteError) -> Error {
    Error::Database(Box::new(err))
}

/// Reads every column of `row` in order.
fn row_columns(row: &Row<'_>, count: usize) -> Result<Vec<Column>, Error> {
    let mut columns = Vec::with_capacity(count);

    for i in 0..count {
        columns.push(from_sqlite(row.get(i).map_err(to_sqlite_error)?));
    }

    Ok(columns)
}

fn all_rows(conn: &Connection, sql: &str, values: Vec<Value>) -> Result<Vec<Vec<Column>>, Error> {
    let mut stmt = prepare(conn, sql, values)?;
    let count = stmt.column_count();
    let mut rows = stmt.raw_query();
    let mut results = vec![];

    while let Some(row) = rows.next().map_err(to_sqlite_error)? {
        results.push(row_columns(row, count)?);
    }

    Ok(results)
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
        let (reply, response) = oneshot::channel();
        let request = Request::IsAutocommit { reply };

        if conn.sender.send(request).is_err() {
            return conn.lost("the sqlite connection task is gone");
        }

        let autocommit = match response.await {
            Ok(result) => result?,
            Err(_) => return conn.lost("the sqlite request did not answer"),
        };

        if autocommit {
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
        let (sql, values) = statement.to_sql()?;
        let (reply, response) = oneshot::channel();
        let request = Request::Execute {
            sql: sql.into(),
            values,
            reply,
        };

        if self.sender.send(request).is_err() {
            return self.lost("the sqlite connection task is gone");
        }

        match response.await {
            Ok(result) => result,
            Err(_) => self.lost("the sqlite request did not answer"),
        }
    }

    async fn fetch_one(&self, statement: Statement) -> Result<Option<Vec<Column>>, Error> {
        let (sql, values) = statement.to_sql()?;
        let (reply, response) = oneshot::channel();
        let request = Request::FetchOne {
            sql: sql.into(),
            values,
            reply,
        };

        if self.sender.send(request).is_err() {
            return self.lost("the sqlite connection task is gone");
        }

        match response.await {
            Ok(result) => result,
            Err(_) => self.lost("the sqlite request did not answer"),
        }
    }

    async fn fetch_all(&self, statement: Statement) -> Result<Vec<Vec<Column>>, Error> {
        let (sql, values) = statement.to_sql()?;
        let (reply, response) = oneshot::channel();
        let request = Request::FetchAll {
            sql: sql.into(),
            values,
            reply,
        };

        if self.sender.send(request).is_err() {
            return self.lost("the sqlite connection task is gone");
        }

        match response.await {
            Ok(result) => result,
            Err(_) => self.lost("the sqlite request did not answer"),
        }
    }

    async fn pluck(&self, statement: Statement) -> Result<Option<Column>, Error> {
        let (sql, values) = statement.to_sql()?;
        let (reply, response) = oneshot::channel();
        let request = Request::Pluck {
            sql: sql.into(),
            values,
            reply,
        };

        if self.sender.send(request).is_err() {
            return self.lost("the sqlite connection task is gone");
        }

        match response.await {
            Ok(result) => result,
            Err(_) => self.lost("the sqlite request did not answer"),
        }
    }

    async fn batch(&self, mut statement: Statement) -> Result<(), Error> {
        let part = statement
            .parts
            .pop()
            .ok_or(Error::Internal("Empty SQL".to_owned()))?;

        let sql = match (statement.parts.is_empty(), part) {
            (true, SqlPart::Raw(sql)) => sql,
            _ => {
                return Err(Error::Internal(
                    "Invalid usage, batch does not support placeholders".to_owned(),
                ))
            }
        };

        let (reply, response) = oneshot::channel();
        let request = Request::Batch { sql, reply };

        if self.sender.send(request).is_err() {
            return self.lost("the sqlite connection task is gone");
        }

        match response.await {
            Ok(result) => result,
            Err(_) => self.lost("the sqlite request did not answer"),
        }
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
        .expect("connection task")
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
        )
        .expect("connection task");

        assert!(matches!(db.provoke_panic().await, Err(Error::Internal(_))));

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
                conn.provoke_panic().await,
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

    /// The connection task cannot be spawned without a runtime, and a pool handing out a
    /// connection whose task never started would hang every query on it.
    #[test]
    fn opening_a_connection_outside_a_runtime_fails() {
        assert!(AsyncSqlite::new(
            Connection::open_in_memory().expect("in memory database"),
            Arc::new(AtomicBool::new(false)),
        )
        .is_err());
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

//! Simple SQLite
use std::convert::identity;
use std::sync::Arc;
use std::{io, thread};

use cdk_common::database::Error;
use cdk_sql_common::database::{DatabaseConnector, DatabaseExecutor, DatabaseTransaction};
use cdk_sql_common::run_db_operation;
use cdk_sql_common::stmt::{query, Column, SqlPart, Statement};
use cdk_sql_common::value::Value;
use rusqlite::{ffi, CachedStatement, Connection, Error as SqliteError, ErrorCode};
use tokio::sync::{mpsc, oneshot};

use crate::common::{from_sqlite, to_sqlite};

/// Piece of work for the thread that owns the connection.
type Job = Box<dyn FnOnce(&Connection) + Send>;

/// Async Sqlite wrapper
#[derive(Debug)]
pub struct AsyncSqlite {
    sender: mpsc::UnboundedSender<Job>,
}

impl AsyncSqlite {
    /// Hands the connection to a dedicated thread, since every rusqlite call blocks and the
    /// connection must stay on one thread to keep its prepared statement cache.
    ///
    /// The thread is deliberately not a tokio blocking task: it parks until the last handle is
    /// dropped, and dropping a runtime waits for every blocking task it owns, so a connection
    /// that outlives its runtime would deadlock shutdown.
    pub fn new(conn: Connection) -> Result<Self, io::Error> {
        let (sender, mut receiver) = mpsc::unbounded_channel::<Job>();

        thread::Builder::new()
            .name("cdk-sqlite".to_owned())
            .spawn(move || {
                while let Some(job) = receiver.blocking_recv() {
                    job(&conn);
                }
            })?;

        Ok(Self { sender })
    }

    /// Runs `f` on the thread owning the connection, timed as `info`, and drops the result with a
    /// log line when the caller is gone, which happens whenever its future is cancelled mid query.
    async fn call<T, F>(&self, info: &str, f: F) -> Result<T, Error>
    where
        F: FnOnce(&Connection) -> Result<T, Error> + Send + 'static,
        T: Send + 'static,
    {
        run_db_operation(
            info,
            async {
                let (reply, response) = oneshot::channel();

                self.sender
                    .send(Box::new(move |conn: &Connection| {
                        if reply.send(f(conn)).is_err() {
                            tracing::debug!("Discarded a sqlite result, its caller is gone");
                        }
                    }))
                    .map_err(|_| Error::Internal("sqlite connection worker is gone".to_owned()))?;

                response.await.map_err(|_| {
                    Error::Internal("sqlite connection worker died mid query".to_owned())
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
        AsyncSqlite::new(Connection::open_in_memory().expect("in memory database"))
            .expect("connection thread")
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

    /// A panicking job unwinds the thread that owns the connection, so every later query on it
    /// must fail instead of waiting for a worker that will never answer.
    #[tokio::test]
    async fn a_dead_worker_fails_queries_instead_of_hanging() {
        let db = in_memory();

        assert!(matches!(
            db.call("panic", |_: &Connection| -> Result<(), Error> {
                panic!("the worker dies here")
            })
            .await,
            Err(Error::Internal(_))
        ));

        assert!(matches!(
            db.pluck(query("SELECT 1").expect("sql")).await,
            Err(Error::Internal(_))
        ));
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
    /// its thread is its own and not one of the runtime's.
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

//! Simple SQLite
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
    inner: Mutex<Connection>,
}

impl AsyncSqlite {
    pub fn new(inner: Connection) -> Self {
        Self {
            inner: inner.into(),
        }
    }
}
impl AsyncSqlite {
    fn get_stmt<'a>(
        &self,
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
        // Nothing to roll back, e.g. when BEGIN never ran. Treating this as an
        // error would discard the connection, which for in-memory databases
        // discards the database itself.
        if conn.inner.get_mut().is_autocommit() {
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
        let conn = self.inner.lock().await;

        let (sql, mut stmt) = self
            .get_stmt(&conn, statement)
            .map_err(|e| Error::Database(Box::new(e)))?;

        run_db_operation_sync(&sql, || stmt.raw_execute(), to_sqlite_error)
    }

    async fn fetch_one(&self, statement: Statement) -> Result<Option<Vec<Column>>, Error> {
        let conn = self.inner.lock().await;
        let (sql, mut stmt) = self
            .get_stmt(&conn, statement)
            .map_err(|e| Error::Database(Box::new(e)))?;

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
    }

    async fn fetch_all(&self, statement: Statement) -> Result<Vec<Vec<Column>>, Error> {
        let conn = self.inner.lock().await;
        let (sql, mut stmt) = self
            .get_stmt(&conn, statement)
            .map_err(|e| Error::Database(Box::new(e)))?;

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
                    )
                }

                Ok(results)
            },
            to_sqlite_error,
        )
    }

    async fn pluck(&self, statement: Statement) -> Result<Option<Column>, Error> {
        let conn = self.inner.lock().await;
        let (sql, mut stmt) = self
            .get_stmt(&conn, statement)
            .map_err(|e| Error::Database(Box::new(e)))?;

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
        let conn = self.inner.lock().await;

        run_db_operation_sync(&sql, || conn.execute_batch(&sql), to_sqlite_error)
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
        AsyncSqlite::new(Connection::open_in_memory().expect("in memory database"))
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

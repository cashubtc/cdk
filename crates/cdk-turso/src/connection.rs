use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use cdk_common::database::Error;
use cdk_sql_common::database::{DatabaseConnector, DatabaseExecutor, DatabaseTransaction};
use cdk_sql_common::stmt::{Column, SqlPart, Statement};
use cdk_sql_common::value::Value;
use tokio::sync::Mutex;

/// A pooled Turso connection.
#[derive(Debug)]
pub struct TursoConnection {
    inner: Mutex<turso::Connection>,
    stale: Arc<AtomicBool>,
}

impl TursoConnection {
    pub(crate) fn new(inner: turso::Connection, stale: Arc<AtomicBool>) -> Self {
        Self {
            inner: Mutex::new(inner),
            stale,
        }
    }

    async fn fetch(&self, statement: Statement, first: bool) -> Result<Vec<Vec<Column>>, Error> {
        let (sql, values) = prepare(statement)?;
        let conn = self.inner.lock().await;
        let previous = self.stale.swap(true, Ordering::SeqCst);
        let result = async {
            let mut rows = conn.query(sql, values).await.map_err(map_error)?;
            let mut results = Vec::new();
            while let Some(row) = rows.next().await.map_err(map_error)? {
                results.push(
                    (0..row.column_count())
                        .map(|i| row.get_value(i).map(from_turso).map_err(map_error))
                        .collect::<Result<Vec<_>, _>>()?,
                );
                if first {
                    break;
                }
            }
            Ok(results)
        }
        .await;
        self.stale.store(previous, Ordering::SeqCst);
        result
    }

    async fn transaction_command(&mut self, sql: &str, finished: bool) -> Result<(), Error> {
        self.stale.store(true, Ordering::SeqCst);
        self.inner
            .get_mut()
            .execute(sql, ())
            .await
            .map_err(map_error)?;
        self.stale.store(!finished, Ordering::SeqCst);
        Ok(())
    }
}

fn prepare(statement: Statement) -> Result<(String, Vec<turso::Value>), Error> {
    let (sql, values) = statement.to_sql()?;
    // BEGIN IMMEDIATE holds the database write lock for the transaction.
    let sql = sql.trim().trim_end_matches("FOR UPDATE").to_owned();
    Ok((sql, values.into_iter().map(to_turso).collect()))
}

fn to_turso(value: Value) -> turso::Value {
    match value {
        Value::Null => turso::Value::Null,
        Value::Integer(value) => turso::Value::Integer(value),
        Value::Real(value) => turso::Value::Real(value),
        Value::Text(value) => turso::Value::Text(value),
        Value::Blob(value) => turso::Value::Blob(value),
    }
}

fn from_turso(value: turso::Value) -> Value {
    match value {
        turso::Value::Null => Value::Null,
        turso::Value::Integer(value) => Value::Integer(value),
        turso::Value::Real(value) => Value::Real(value),
        turso::Value::Text(value) => Value::Text(value),
        turso::Value::Blob(value) => Value::Blob(value),
    }
}

fn map_error(error: turso::Error) -> Error {
    match &error {
        // Turso exposes constraint diagnostics rather than extended error codes.
        turso::Error::Constraint(message)
            if message.starts_with("UNIQUE constraint failed:")
                || message.starts_with("PRIMARY KEY constraint failed:") =>
        {
            Error::Duplicate
        }
        _ => Error::Database(Box::new(error)),
    }
}

/// Transaction handling for Turso's single-writer database.
#[derive(Debug)]
pub struct TursoTransaction;

#[async_trait::async_trait]
impl DatabaseTransaction<TursoConnection> for TursoTransaction {
    async fn begin(conn: &mut TursoConnection) -> Result<(), Error> {
        conn.transaction_command("BEGIN IMMEDIATE", false).await
    }

    async fn commit(conn: &mut TursoConnection) -> Result<(), Error> {
        conn.transaction_command("COMMIT", true).await
    }

    async fn rollback(conn: &mut TursoConnection) -> Result<(), Error> {
        conn.transaction_command("ROLLBACK", true).await
    }
}

impl DatabaseConnector for TursoConnection {
    type Transaction = TursoTransaction;
}

#[async_trait::async_trait]
impl DatabaseExecutor for TursoConnection {
    fn name() -> &'static str {
        // Reuse the SQLite dialect and migration history.
        "sqlite"
    }

    async fn execute(&self, statement: Statement) -> Result<usize, Error> {
        let (sql, values) = prepare(statement)?;
        let conn = self.inner.lock().await;
        let previous = self.stale.swap(true, Ordering::SeqCst);
        let result = conn.execute(sql, values).await.map_err(map_error);
        self.stale.store(previous, Ordering::SeqCst);
        usize::try_from(result?).map_err(|err| Error::Database(Box::new(err)))
    }

    async fn fetch_one(&self, statement: Statement) -> Result<Option<Vec<Column>>, Error> {
        Ok(self.fetch(statement, true).await?.into_iter().next())
    }

    async fn fetch_all(&self, statement: Statement) -> Result<Vec<Vec<Column>>, Error> {
        self.fetch(statement, false).await
    }

    async fn pluck(&self, statement: Statement) -> Result<Option<Column>, Error> {
        Ok(self
            .fetch_one(statement)
            .await?
            .and_then(|row| row.into_iter().next()))
    }

    async fn batch(&self, statement: Statement) -> Result<(), Error> {
        let sql = match statement.parts.as_slice() {
            [SqlPart::Raw(sql)] => sql,
            _ => {
                return Err(Error::Internal(
                    "Batch requires raw SQL without placeholders".to_owned(),
                ))
            }
        };
        let conn = self.inner.lock().await;
        let previous = self.stale.swap(true, Ordering::SeqCst);
        let result = conn.execute_batch(sql.as_ref()).await.map_err(map_error);
        self.stale.store(previous, Ordering::SeqCst);
        result
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use cdk_sql_common::database::ConnectionWithTransaction;
    use cdk_sql_common::pool::Pool;
    use cdk_sql_common::stmt::query;

    use super::*;
    use crate::{Config, TursoConnectionManager};

    #[tokio::test]
    async fn binds_values_and_classifies_constraints() {
        let config = Config::new(":memory:").await.expect("database");
        let pool = Pool::<TursoConnectionManager>::new(config);
        let conn = pool.get().await.expect("connection");
        query("CREATE TABLE values_test (id INTEGER PRIMARY KEY, text TEXT UNIQUE NOT NULL, data BLOB, number REAL, optional TEXT)")
            .expect("query").execute(&*conn).await.expect("table");
        let insert = || {
            query("INSERT INTO values_test VALUES (:id, :text, :data, :number, :optional)")
                .expect("query")
                .bind("id", 1i64)
                .bind("text", "'quoted'; :parameter")
                .bind("data", vec![0u8, 255, 0])
                .bind("number", Value::Real(1.25))
                .bind("optional", Value::Null)
        };
        assert_eq!(insert().execute(&*conn).await.expect("insert"), 1);
        assert!(matches!(
            insert().execute(&*conn).await,
            Err(Error::Duplicate)
        ));
        let row = query("SELECT * FROM values_test")
            .expect("query")
            .fetch_one(&*conn)
            .await
            .expect("read")
            .expect("row");
        assert!(
            matches!(&row[..], [Value::Integer(1), Value::Text(text), Value::Blob(blob), Value::Real(1.25), Value::Null]
            if text == "'quoted'; :parameter" && blob == &[0, 255, 0])
        );
        let error = query("INSERT INTO values_test (id) VALUES (2)")
            .expect("query")
            .execute(&*conn)
            .await
            .expect_err("not-null constraint");
        assert!(matches!(error, Error::Database(_)));
    }

    #[tokio::test]
    async fn transactions_commit_rollback_and_rollback_on_drop() {
        let config = Config::new(":memory:").await.expect("database");
        let pool = Pool::<TursoConnectionManager>::new(config);
        {
            let conn = pool.get().await.expect("connection");
            query("CREATE TABLE test (id INTEGER PRIMARY KEY)")
                .expect("query")
                .execute(&*conn)
                .await
                .expect("table");
        }
        let tx = ConnectionWithTransaction::new(pool.get().await.expect("connection"))
            .await
            .expect("transaction");
        query("INSERT INTO test VALUES (1)")
            .expect("query")
            .execute(&tx)
            .await
            .expect("insert");
        tx.commit().await.expect("commit");

        let tx = ConnectionWithTransaction::new(pool.get().await.expect("connection"))
            .await
            .expect("transaction");
        query("INSERT INTO test VALUES (2)")
            .expect("query")
            .execute(&tx)
            .await
            .expect("insert");
        tx.rollback().await.expect("rollback");

        let tx = ConnectionWithTransaction::new(pool.get().await.expect("connection"))
            .await
            .expect("transaction");
        query("INSERT INTO test VALUES (3)")
            .expect("query")
            .execute(&tx)
            .await
            .expect("insert");
        drop(tx);
        let conn = pool
            .get_timeout(Duration::from_secs(2))
            .await
            .expect("connection returned");
        assert!(matches!(
            query("SELECT SUM(id) FROM test")
                .expect("query")
                .pluck(&*conn)
                .await
                .expect("read"),
            Some(Value::Integer(1))
        ));
    }

    #[tokio::test]
    async fn discards_unfinished_transactions_without_losing_memory_database() {
        let config = Config::new(":memory:").await.expect("database");
        let pool = Pool::<TursoConnectionManager>::new(config);
        {
            let mut conn = pool.get().await.expect("connection");
            query("CREATE TABLE test (id INTEGER)")
                .expect("query")
                .execute(&*conn)
                .await
                .expect("table");
            TursoTransaction::begin(&mut conn).await.expect("begin");
            query("INSERT INTO test VALUES (1)")
                .expect("query")
                .execute(&*conn)
                .await
                .expect("insert");
            // Simulate cancellation before the transaction wrapper takes ownership.
        }
        let conn = pool.get().await.expect("replacement connection");
        assert!(matches!(
            query("SELECT COUNT(*) FROM test")
                .expect("query")
                .pluck(&*conn)
                .await
                .expect("read"),
            Some(Value::Integer(0))
        ));
    }
}

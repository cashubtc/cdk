use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cdk_sql_common::pool::DatabasePool;
use cdk_sql_common::stmt::query;
use rusqlite::Connection;

use super::TestDirectory;
use crate::common::{Config, SqliteConnectionManager};

#[tokio::test(flavor = "current_thread")]
async fn contended_sqlite_write_leaves_executor_available_to_release_holder() {
    let directory = TestDirectory::new();
    let path = directory.0.join("wallet.sqlite");
    // Use the real manager, including its ten-second busy timeout and FULL/WAL
    // settings. Both connections are created before taking the write lock.
    let connection = Arc::new(
        SqliteConnectionManager::new_resource(
            &Config::from(&path),
            Arc::new(AtomicBool::new(false)),
            Duration::from_secs(5),
        )
        .unwrap(),
    );
    let holder = Connection::open(&path).unwrap();
    holder
        .execute_batch("CREATE TABLE contention (id INTEGER PRIMARY KEY); BEGIN IMMEDIATE; INSERT INTO contention VALUES (1)")
        .unwrap();

    let (started, waiting) = tokio::sync::oneshot::channel();
    let start = Instant::now();
    let writer = tokio::spawn(async move {
        started.send(()).unwrap();
        query("INSERT INTO contention VALUES (2)")
            .unwrap()
            .execute(connection.as_ref())
            .await
    });
    // Sending the signal does not yield the writer. An inline rusqlite busy
    // wait therefore starves this same-thread COMMIT until its timeout expires.
    waiting.await.unwrap();
    holder.execute_batch("COMMIT").unwrap();
    let result = writer.await.unwrap();
    assert!(
        result.is_ok(),
        "SQLite blocked the executor needed to release its writer: elapsed={:?}, result={result:?}",
        start.elapsed()
    );
    assert!(start.elapsed() < Duration::from_secs(2));
    let count: u64 = holder
        .query_row("SELECT COUNT(*) FROM contention", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 2);
}

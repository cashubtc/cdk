use std::sync::mpsc::channel;
use std::thread;
use std::time::{Duration, Instant};

use cdk_sql_common::database::ConnectionWithTransaction;
use cdk_sql_common::pool::Pool;
use tokio::runtime::Builder;
use tokio::task::JoinSet;
use tokio::time::sleep;

use super::tests::TestDirectory;
use super::*;
use crate::common::{Config, SqliteConnectionManager};

fn in_memory() -> AsyncSqlite {
    AsyncSqlite::new(
        Connection::open_in_memory().expect("in memory database"),
        Arc::new(AtomicBool::new(false)),
    )
}

fn config_for(path: &str) -> Config {
    #[cfg(not(feature = "sqlcipher"))]
    let config: Config = path.into();
    #[cfg(feature = "sqlcipher")]
    let config: Config = (path, "test".parse().expect("non-empty key")).into();

    config
}

/// Runs `writers` write transactions at once, each keeping the write lock for `statements`
/// inserts plus an optional suspension, and reports what every writer got.
async fn write_burst(
    writers: usize,
    statements: usize,
    hold: Option<Duration>,
) -> Vec<Result<(), Error>> {
    let directory = TestDirectory::new();
    let path = directory
        .0
        .join("write-burst.sqlite")
        .to_str()
        .expect("utf8 path")
        .to_owned();
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
    let directory = TestDirectory::new();
    let path = directory
        .0
        .join("close-before-backup.sqlite")
        .to_str()
        .expect("utf8 path")
        .to_owned();
    let backup = directory
        .0
        .join("closed-backup.sqlite")
        .to_str()
        .expect("utf8 path")
        .to_owned();
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

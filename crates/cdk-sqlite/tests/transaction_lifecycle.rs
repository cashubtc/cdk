//! Transaction lifecycle tests against SQLite and the shared connection pool.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cdk_common::database::Error;
use cdk_sql_common::database::{
    ConnectionWithTransaction, DatabaseConnector, DatabaseExecutor, GenericTransactionHandler,
};
use cdk_sql_common::pool::{self, DatabaseConfig, DatabasePool, Pool};
use cdk_sql_common::stmt::{query, Column, Statement};
use cdk_sqlite::SqliteConnectionManager;
use tokio::sync::Notify;

#[derive(Debug, Default)]
struct Controls {
    begin_wait: AtomicBool,
    begin_before_execution_wait: AtomicBool,
    begin_fail: AtomicBool,
    commit_wait: AtomicBool,
    commit_fail: AtomicBool,
    commit_ack_wait: AtomicBool,
    commit_ack_fail: AtomicBool,
    rollback_wait: AtomicBool,
    rollback_fail: AtomicBool,
    began: Notify,
    committing: Notify,
    rolling_back: Notify,
    closed: Notify,
    created: AtomicUsize,
}

#[derive(Debug, Clone)]
struct Config {
    path: PathBuf,
    controls: Arc<Controls>,
}

impl DatabaseConfig for Config {
    fn max_size(&self) -> usize {
        1
    }

    fn default_timeout(&self) -> Duration {
        Duration::from_secs(10)
    }
}

#[derive(Debug)]
struct Connection {
    inner: <SqliteConnectionManager as DatabasePool>::Connection,
    controls: Arc<Controls>,
    id: usize,
    stale: Arc<AtomicBool>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.controls.closed.notify_one();
    }
}

#[async_trait::async_trait]
impl DatabaseExecutor for Connection {
    fn name() -> &'static str {
        "sqlite"
    }

    async fn execute(&self, statement: Statement) -> Result<usize, Error> {
        let (sql, parameters) = statement.to_sql()?;
        assert!(parameters.is_empty());
        match sql.trim() {
            "START TRANSACTION" => {
                if self
                    .controls
                    .begin_before_execution_wait
                    .load(Ordering::SeqCst)
                {
                    self.controls.began.notify_one();
                    std::future::pending::<()>().await;
                }
                self.inner.execute(query("BEGIN IMMEDIATE")?).await?;
                self.controls.began.notify_one();
                if self.controls.begin_wait.load(Ordering::SeqCst) {
                    std::future::pending::<()>().await;
                }
                if self.controls.begin_fail.load(Ordering::SeqCst) {
                    return Err(Error::Internal("begin acknowledgement failed".to_owned()));
                }
                Ok(0)
            }
            "COMMIT" => {
                let already_committed = self.controls.commit_ack_wait.load(Ordering::SeqCst)
                    || self.controls.commit_ack_fail.load(Ordering::SeqCst);
                if already_committed {
                    self.inner.execute(query(&sql)?).await?;
                }
                self.controls.committing.notify_one();
                if self.controls.commit_wait.load(Ordering::SeqCst)
                    || self.controls.commit_ack_wait.load(Ordering::SeqCst)
                {
                    std::future::pending::<()>().await;
                }
                if self.controls.commit_fail.load(Ordering::SeqCst)
                    || self.controls.commit_ack_fail.load(Ordering::SeqCst)
                {
                    return Err(Error::Internal("commit failed".to_owned()));
                }
                self.inner.execute(query(&sql)?).await
            }
            "ROLLBACK" => {
                self.controls.rolling_back.notify_one();
                if self.controls.rollback_wait.load(Ordering::SeqCst) {
                    std::future::pending::<()>().await;
                }
                if self.controls.rollback_fail.load(Ordering::SeqCst) {
                    return Err(Error::Internal("rollback failed".to_owned()));
                }
                self.inner.execute(query(&sql)?).await
            }
            _ => self.inner.execute(query(&sql)?).await,
        }
    }

    async fn fetch_one(&self, s: Statement) -> Result<Option<Vec<Column>>, Error> {
        self.inner.fetch_one(s).await
    }

    async fn fetch_all(&self, s: Statement) -> Result<Vec<Vec<Column>>, Error> {
        self.inner.fetch_all(s).await
    }

    async fn pluck(&self, s: Statement) -> Result<Option<Column>, Error> {
        self.inner.pluck(s).await
    }

    async fn batch(&self, s: Statement) -> Result<(), Error> {
        self.inner.batch(s).await
    }
}

impl DatabaseConnector for Connection {
    type Transaction = GenericTransactionHandler<Self>;
}

#[derive(Debug)]
struct Manager;

impl DatabasePool for Manager {
    type Connection = Connection;
    type Config = Config;
    type Error = rusqlite::Error;

    fn new_resource(
        config: &Config,
        stale: Arc<AtomicBool>,
        timeout: Duration,
    ) -> Result<Connection, pool::Error<Self::Error>> {
        Ok(Connection {
            inner: SqliteConnectionManager::new_resource(
                &config.path.clone().into(),
                stale.clone(),
                timeout,
            )?,
            controls: config.controls.clone(),
            id: config.controls.created.fetch_add(1, Ordering::SeqCst),
            stale,
        })
    }
}

async fn setup() -> (Config, Arc<Pool<Manager>>) {
    let config = Config {
        path: std::env::temp_dir().join(format!("cdk-transaction-{}.sqlite", uuid::Uuid::new_v4())),
        controls: Arc::default(),
    };
    let pool = Pool::<Manager>::new(config.clone());
    let conn = pool.get().await.expect("connection");
    query("CREATE TABLE accounting (value INTEGER)")
        .expect("query")
        .execute(&*conn)
        .await
        .expect("create table");
    drop(conn);
    (config, pool)
}

async fn assert_clean(pool: &Arc<Pool<Manager>>, path: &PathBuf, expected: i64) {
    let conn = pool.get().await.expect("pool slot must remain available");
    let rows = conn
        .fetch_all(query("SELECT COUNT(*) FROM accounting").expect("query"))
        .await
        .expect("read");
    let observer = rusqlite::Connection::open(path).expect("independent observer");
    let durable: i64 = observer
        .query_row("SELECT COUNT(*) FROM accounting", [], |r| r.get(0))
        .expect("durable read");
    assert_eq!(durable, expected);
    assert_eq!(rows, vec![vec![Column::Integer(expected)]]);
    conn.inner
        .execute(query("BEGIN IMMEDIATE").expect("query"))
        .await
        .expect("no active transaction may return to the pool");
    conn.inner
        .execute(query("ROLLBACK").expect("query"))
        .await
        .expect("close probe");
}

#[tokio::test]
async fn cancelled_begin_after_execution_before_acknowledgement() {
    let (config, pool) = setup().await;
    config.controls.begin_wait.store(true, Ordering::SeqCst);
    let opening = tokio::spawn(ConnectionWithTransaction::new(
        pool.get().await.expect("conn"),
    ));
    config.controls.began.notified().await;
    opening.abort();
    assert!(opening.await.expect_err("aborted").is_cancelled());
    assert_clean(&pool, &config.path, 0).await;
}

#[tokio::test]
async fn failed_begin_acknowledgement_cannot_recycle_active_transaction() {
    let (config, pool) = setup().await;
    config.controls.begin_fail.store(true, Ordering::SeqCst);
    assert!(
        ConnectionWithTransaction::new(pool.get().await.expect("conn"))
            .await
            .is_err()
    );
    assert_clean(&pool, &config.path, 0).await;
}

#[tokio::test]
async fn persistent_rollback_failure_discards_and_releases_pool_capacity() {
    let (config, pool) = setup().await;
    config.controls.rollback_fail.store(true, Ordering::SeqCst);
    for _ in 0..25 {
        let tx = ConnectionWithTransaction::new(pool.get().await.expect("conn"))
            .await
            .expect("begin");
        tx.execute(query("INSERT INTO accounting VALUES (8)").expect("query"))
            .await
            .expect("write");
        assert!(tx.rollback().await.is_err());
        tokio::time::timeout(Duration::from_secs(2), config.controls.closed.notified())
            .await
            .expect("failed cleanup must close the connection");
        assert_clean(&pool, &config.path, 0).await;
    }
    assert_eq!(config.controls.created.load(Ordering::SeqCst), 26);
}

#[test]
fn shutdown_before_cleanup_poll_and_during_cleanup_cannot_recycle() {
    for poll_cleanup in [false, true] {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let (config, pool) = runtime.block_on(setup());
        config.controls.rollback_wait.store(true, Ordering::SeqCst);
        runtime.block_on(async {
            let tx = ConnectionWithTransaction::new(pool.get().await.expect("conn"))
                .await
                .expect("begin");
            tx.execute(query("INSERT INTO accounting VALUES (8)").expect("query"))
                .await
                .expect("write");
            drop(tx);
            if poll_cleanup {
                tokio::time::timeout(
                    Duration::from_secs(2),
                    config.controls.rolling_back.notified(),
                )
                .await
                .expect("cleanup must enter rollback");
            }
        });
        drop(runtime);
        let next_runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        next_runtime.block_on(assert_clean(&pool, &config.path, 0));
        assert_eq!(config.controls.created.load(Ordering::SeqCst), 2);
    }
}

#[tokio::test]
async fn successful_commit_and_rollback_reuse_healthy_connection() {
    let (config, pool) = setup().await;
    for commit in [true, false] {
        let conn = pool.get().await.expect("conn");
        assert_eq!(conn.id, 0);
        let tx = ConnectionWithTransaction::new(conn).await.expect("begin");
        tx.execute(query("INSERT INTO accounting VALUES (8)").expect("query"))
            .await
            .expect("write");
        if commit {
            tx.commit().await.expect("commit");
        } else {
            tx.rollback().await.expect("rollback");
        }
        assert_clean(&pool, &config.path, 1).await;
    }
    assert_eq!(config.controls.created.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn failed_and_cancelled_commit_remain_isolated() {
    for cancel in [false, true] {
        let (config, pool) = setup().await;
        config.controls.commit_wait.store(cancel, Ordering::SeqCst);
        config.controls.commit_fail.store(!cancel, Ordering::SeqCst);
        let tx = ConnectionWithTransaction::new(pool.get().await.expect("conn"))
            .await
            .expect("begin");
        tx.execute(query("INSERT INTO accounting VALUES (8)").expect("query"))
            .await
            .expect("write");
        if cancel {
            let committing = tokio::spawn(tx.commit());
            config.controls.committing.notified().await;
            committing.abort();
            assert!(committing.await.expect_err("aborted").is_cancelled());
        } else {
            assert!(tx.commit().await.is_err());
        }
        assert_clean(&pool, &config.path, 0).await;
    }
}

#[tokio::test]
async fn panicking_transaction_owner_remains_isolated() {
    let (config, pool) = setup().await;
    let tx = ConnectionWithTransaction::new(pool.get().await.expect("conn"))
        .await
        .expect("begin");
    tx.execute(query("INSERT INTO accounting VALUES (8)").expect("query"))
        .await
        .expect("write");
    let task = tokio::spawn(async move {
        let _tx = tx;
        panic!("owner panicked");
    });
    assert!(task.await.expect_err("panic").is_panic());
    assert_clean(&pool, &config.path, 0).await;
}

#[test]
fn dropping_transaction_without_runtime_discards_connection() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let (config, pool) = runtime.block_on(setup());
    let tx = runtime.block_on(async {
        let tx = ConnectionWithTransaction::new(pool.get().await.expect("conn"))
            .await
            .expect("begin");
        tx.execute(query("INSERT INTO accounting VALUES (8)").expect("query"))
            .await
            .expect("write");
        tx
    });
    drop(tx);
    runtime.block_on(assert_clean(&pool, &config.path, 0));
    assert_eq!(config.controls.created.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn rollback_cleanup_deadline_releases_pool_slot() {
    let (config, pool) = setup().await;
    config.controls.rollback_wait.store(true, Ordering::SeqCst);
    let tx = ConnectionWithTransaction::new(pool.get().await.expect("conn"))
        .await
        .expect("begin");
    tx.execute(query("INSERT INTO accounting VALUES (8)").expect("query"))
        .await
        .expect("write");
    drop(tx);
    tokio::time::timeout(
        Duration::from_secs(2),
        config.controls.rolling_back.notified(),
    )
    .await
    .expect("cleanup must enter rollback");
    assert!(matches!(
        pool.get_timeout(Duration::from_millis(20)).await,
        Err(pool::Error::Timeout)
    ));
    assert_clean(&pool, &config.path, 0).await;
    assert_eq!(config.controls.created.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn cancelled_explicit_rollback_discards_if_cleanup_still_fails() {
    let (config, pool) = setup().await;
    config.controls.rollback_wait.store(true, Ordering::SeqCst);
    let tx = ConnectionWithTransaction::new(pool.get().await.expect("conn"))
        .await
        .expect("begin");
    tx.execute(query("INSERT INTO accounting VALUES (8)").expect("query"))
        .await
        .expect("write");
    let rollback = tokio::spawn(tx.rollback());
    tokio::time::timeout(
        Duration::from_secs(2),
        config.controls.rolling_back.notified(),
    )
    .await
    .expect("explicit rollback must enter driver");
    rollback.abort();
    assert!(rollback.await.expect_err("aborted").is_cancelled());
    assert_clean(&pool, &config.path, 0).await;
    assert_eq!(config.controls.created.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn successful_commit_does_not_clear_driver_staleness() {
    let (config, pool) = setup().await;
    let conn = pool.get().await.expect("conn");
    conn.stale.store(true, Ordering::SeqCst);
    let tx = ConnectionWithTransaction::new(conn).await.expect("begin");
    tx.commit().await.expect("commit");
    assert_clean(&pool, &config.path, 0).await;
    assert_eq!(config.controls.created.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn successful_drop_cleanup_reuses_healthy_connection() {
    let (config, pool) = setup().await;
    let tx = ConnectionWithTransaction::new(pool.get().await.expect("conn"))
        .await
        .expect("begin");
    tx.execute(query("INSERT INTO accounting VALUES (8)").expect("query"))
        .await
        .expect("write");
    drop(tx);
    assert_clean(&pool, &config.path, 0).await;
    assert_eq!(config.controls.created.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn unpolled_opening_future_does_not_start_transaction() {
    let (config, pool) = setup().await;
    let opening = ConnectionWithTransaction::new(pool.get().await.expect("conn"));
    drop(opening);
    assert_clean(&pool, &config.path, 0).await;
    assert_eq!(config.controls.created.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancelled_begin_before_execution_discards_uncertain_resource() {
    let (config, pool) = setup().await;
    config
        .controls
        .begin_before_execution_wait
        .store(true, Ordering::SeqCst);
    let opening = tokio::spawn(ConnectionWithTransaction::new(
        pool.get().await.expect("conn"),
    ));
    config.controls.began.notified().await;
    opening.abort();
    assert!(opening.await.expect_err("aborted").is_cancelled());
    assert_clean(&pool, &config.path, 0).await;
    assert_eq!(config.controls.created.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn lost_commit_acknowledgement_preserves_durable_commit_without_dirty_reuse() {
    for cancel in [false, true] {
        let (config, pool) = setup().await;
        config
            .controls
            .commit_ack_wait
            .store(cancel, Ordering::SeqCst);
        config
            .controls
            .commit_ack_fail
            .store(!cancel, Ordering::SeqCst);
        let tx = ConnectionWithTransaction::new(pool.get().await.expect("conn"))
            .await
            .expect("begin");
        tx.execute(query("INSERT INTO accounting VALUES (8)").expect("query"))
            .await
            .expect("write");
        if cancel {
            let commit = tokio::spawn(tx.commit());
            config.controls.committing.notified().await;
            commit.abort();
            assert!(commit.await.expect_err("aborted").is_cancelled());
        } else {
            assert!(tx.commit().await.is_err());
        }
        assert_clean(&pool, &config.path, 1).await;
        assert_eq!(config.controls.created.load(Ordering::SeqCst), 2);
    }
}

#[tokio::test]
async fn explicit_rollback_error_discards_even_if_retry_would_succeed() {
    let (config, pool) = setup().await;
    config.controls.rollback_fail.store(true, Ordering::SeqCst);
    let tx = ConnectionWithTransaction::new(pool.get().await.expect("conn"))
        .await
        .expect("begin");
    tx.execute(query("INSERT INTO accounting VALUES (8)").expect("query"))
        .await
        .expect("write");
    assert!(tx.rollback().await.is_err());
    config.controls.rollback_fail.store(false, Ordering::SeqCst);
    assert_clean(&pool, &config.path, 0).await;
    assert_eq!(config.controls.created.load(Ordering::SeqCst), 2);
}

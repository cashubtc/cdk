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
        let path = std::env::temp_dir().join(format!("cdk-contention-{}", uuid::Uuid::new_v4()));
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
                Release::Fail => return Err(Error::Internal("injected driver failure".to_owned())),
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

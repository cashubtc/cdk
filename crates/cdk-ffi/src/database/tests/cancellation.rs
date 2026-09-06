use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use cdk_common::database::Error;
use cdk_sql_common::database::{DatabaseExecutor, SqlBackend, SqlConnection, SqlTransaction};
use cdk_sql_common::stmt::query;
use cdk_sql_common::SQLWalletDatabase;
use cdk_sqlite::{SqliteBackend, SqliteConnection, SqliteTransaction};

use super::{run, FfiWalletDatabaseWrapper, WakeThread, WalletDatabase};

#[derive(Debug, Clone)]
struct Config {
    path: String,
    creates: Arc<AtomicUsize>,
}

/// SQLite backend that counts physical connections.
///
/// TEMP tables are private to one SQLite connection, so a missing marker
/// identifies a connection that has not been checked out before.
#[derive(Debug, Clone)]
struct CountingBackend {
    inner: SqliteBackend,
    creates: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl SqlBackend for CountingBackend {
    type Config = Config;
    type Connection = SqliteConnection;
    type Transaction = SqliteTransaction;

    fn new(config: Config) -> Result<Self, Error> {
        Ok(Self {
            inner: SqliteBackend::new(config.path.as_str().into())?,
            creates: config.creates,
        })
    }

    async fn acquire(&self) -> Result<SqliteConnection, Error> {
        let conn = self.inner.acquire().await?;
        let marked = conn
            .pluck(query(
                "SELECT count(*) FROM sqlite_temp_master WHERE name = 'connection_marker'",
            )?)
            .await?;
        if !matches!(marked, Some(cdk_sql_common::stmt::Column::Integer(1))) {
            self.creates.fetch_add(1, Ordering::SeqCst);
            conn.execute(query("CREATE TEMP TABLE connection_marker (id INTEGER)")?)
                .await?;
        }
        Ok(conn)
    }

    async fn close(&self) -> Result<(), Error> {
        self.inner.close().await
    }
}

#[test]
fn cancelled_database_trait_call_cleans_up_outside_tokio() {
    let path = std::env::temp_dir().join(format!("cdk-ffi-cancel-{}.sqlite", uuid::Uuid::new_v4()));
    let config = Config {
        path: path.to_str().unwrap().to_owned(),
        creates: Arc::default(),
    };
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let native = runtime
        .block_on(SQLWalletDatabase::<CountingBackend>::new(config.clone()))
        .unwrap();
    let foreign: Arc<dyn WalletDatabase> = FfiWalletDatabaseWrapper::new(native);
    let blocker = runtime
        .block_on(async { SqliteBackend::new(config.path.as_str().into()) })
        .unwrap();
    let transaction = runtime.block_on(async {
        blocker
            .acquire()
            .await
            .unwrap()
            .begin_transaction()
            .await
            .unwrap()
    });

    assert!(tokio::runtime::Handle::try_current().is_err());
    let waker = std::task::Waker::from(Arc::new(WakeThread(std::thread::current())));
    let mut context = std::task::Context::from_waker(&waker);
    let mut pending = foreign.update_proofs(vec![], vec![]);
    // The warmed single-connection pool reaches BEGIN on the first poll. The
    // independent writer keeps BEGIN pending until after the foreign caller cancels.
    assert!(pending.as_mut().poll(&mut context).is_pending());
    assert!(tokio::runtime::Handle::try_current().is_err());
    drop(pending);
    assert!(tokio::runtime::Handle::try_current().is_err());
    runtime.block_on(transaction.rollback()).unwrap();

    // Checkout waits for rollback. Without Tokio during future destruction,
    // cleanup discards the connection and this would require a second open.
    run(foreign.kv_write(
        "review".to_owned(),
        "".to_owned(),
        "key".to_owned(),
        vec![42],
    ))
    .unwrap();
    assert_eq!(
        run(foreign.kv_read("review".to_owned(), "".to_owned(), "key".to_owned())).unwrap(),
        Some(vec![42])
    );
    assert_eq!(config.creates.load(Ordering::SeqCst), 1);

    drop(foreign);
    drop(blocker);
    drop(runtime);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", config.path));
    }
}

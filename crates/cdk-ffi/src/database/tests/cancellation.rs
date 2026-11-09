use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cdk_sql_common::database::ConnectionWithTransaction;
use cdk_sql_common::pool::{self, DatabaseConfig, DatabasePool, Pool};
use cdk_sql_common::SQLWalletDatabase;
use cdk_sqlite::SqliteConnectionManager;

use super::{run, FfiWalletDatabaseWrapper, WakeThread, WalletDatabase};

#[derive(Debug, Clone)]
struct Config {
    path: String,
    creates: Arc<AtomicUsize>,
}

impl DatabaseConfig for Config {
    fn max_size(&self) -> usize {
        1
    }

    fn default_timeout(&self) -> Duration {
        Duration::from_secs(5)
    }
}

#[derive(Debug)]
struct Manager;

impl DatabasePool for Manager {
    type Config = Config;
    type Connection = <SqliteConnectionManager as DatabasePool>::Connection;
    type Error = <SqliteConnectionManager as DatabasePool>::Error;

    fn new_resource(
        config: &Config,
        stale: Arc<AtomicBool>,
        timeout: Duration,
    ) -> Result<Self::Connection, pool::Error<Self::Error>> {
        config.creates.fetch_add(1, Ordering::SeqCst);
        SqliteConnectionManager::new_resource(&config.path.as_str().into(), stale, timeout)
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
        .block_on(SQLWalletDatabase::<Manager>::new(config.clone()))
        .unwrap();
    let foreign: Arc<dyn WalletDatabase> = FfiWalletDatabaseWrapper::new(native);
    let blocker = Pool::<SqliteConnectionManager>::new(config.path.as_str().into());
    let transaction = runtime.block_on(async {
        ConnectionWithTransaction::new(blocker.get().await.unwrap())
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

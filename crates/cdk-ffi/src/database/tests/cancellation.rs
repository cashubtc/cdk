use std::sync::Arc;

use cdk_sql_common::database::{SqlBackend, SqlConnection, SqlTransaction};
use cdk_sqlite::{SqliteBackend, WalletSqliteDatabase};

use super::{run, FfiWalletDatabaseWrapper, WakeThread, WalletDatabase};

#[test]
fn cancelled_database_trait_call_cleans_up_outside_tokio() {
    let path = std::env::temp_dir().join(format!("cdk-ffi-cancel-{}.sqlite", uuid::Uuid::new_v4()));
    let path = path.to_str().unwrap().to_owned();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let native = runtime
        .block_on(WalletSqliteDatabase::new(path.as_str()))
        .unwrap();
    let foreign: Arc<dyn WalletDatabase> = FfiWalletDatabaseWrapper::new(native);
    let blocker = runtime
        .block_on(async { SqliteBackend::new(path.as_str().into()) })
        .unwrap();
    let begin_blocker = || {
        runtime.block_on(async {
            blocker
                .acquire()
                .await
                .unwrap()
                .begin_transaction()
                .await
                .unwrap()
        })
    };
    let transaction = begin_blocker();

    assert!(tokio::runtime::Handle::try_current().is_err());
    let waker = std::task::Waker::from(Arc::new(WakeThread(std::thread::current())));
    let mut context = std::task::Context::from_waker(&waker);
    let mut pending = foreign.update_proofs(vec![], vec![]);
    // The independent writer keeps BEGIN pending until after the foreign
    // caller cancels.
    assert!(pending.as_mut().poll(&mut context).is_pending());
    assert!(tokio::runtime::Handle::try_current().is_err());
    drop(pending);
    assert!(tokio::runtime::Handle::try_current().is_err());
    runtime.block_on(transaction.rollback()).unwrap();

    // Cleanup must end the cancelled transaction once its BEGIN completes. A
    // leaked transaction would keep the write lock and time out this writer.
    runtime.block_on(begin_blocker().commit()).unwrap();
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

    drop(foreign);
    drop(blocker);
    drop(runtime);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{path}{suffix}"));
    }
}

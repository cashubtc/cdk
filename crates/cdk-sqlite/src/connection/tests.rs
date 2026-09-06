use std::sync::mpsc;

use cdk_sql_common::database::SqlBackend;
use cdk_sql_common::stmt::query;
use cdk_sql_common::value::Value;

use super::*;
use crate::{Config, SqliteBackend};

async fn database() -> SqliteBackend {
    let backend = SqliteBackend::new(Config::from(":memory:")).unwrap();
    backend
        .acquire()
        .await
        .unwrap()
        .batch(query("CREATE TABLE test (id INTEGER PRIMARY KEY)").unwrap())
        .await
        .unwrap();
    backend
}

async fn count(backend: &SqliteBackend) -> i64 {
    let conn = backend.acquire().await.unwrap();
    assert!(conn.run(|c| Ok(c.is_autocommit())).await.unwrap());
    match conn
        .pluck(query("SELECT COUNT(*) FROM test").unwrap())
        .await
        .unwrap()
    {
        Some(Value::Integer(n)) => n,
        other => panic!("unexpected count: {other:?}"),
    }
}

#[tokio::test]
async fn commit_rollback_drop_and_error_do_not_leak_transactions() {
    let backend = database().await;
    for finish in 0_i64..4 {
        let tx = backend.begin_transaction().await.unwrap();
        tx.execute(
            query("INSERT INTO test VALUES (:id)")
                .unwrap()
                .bind("id", finish),
        )
        .await
        .unwrap();
        match finish {
            0 => tx.commit().await.unwrap(),
            1 => tx.rollback().await.unwrap(),
            2 => drop(tx),
            _ => {
                assert!(tx
                    .execute(query("INSERT INTO test VALUES (0)").unwrap())
                    .await
                    .is_err());
                drop(tx);
            }
        }
        assert_eq!(count(&backend).await, 1);
    }
}

#[tokio::test]
async fn cancelled_blocking_query_retains_checkout_until_rollback() {
    let backend = database().await;
    let tx = Arc::new(backend.begin_transaction().await.unwrap());
    let (started, entered) = oneshot::channel();
    let (release, blocked) = mpsc::channel();
    let task = tokio::spawn({
        let tx = tx.clone();
        async move {
            tx.0.run(move |conn| {
                conn.execute_batch("INSERT INTO test VALUES (1)").unwrap();
                started.send(()).unwrap();
                blocked.recv().unwrap();
                Ok(())
            })
            .await
        }
    });
    entered.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(
        tokio::time::timeout(Duration::from_millis(30), backend.acquire())
            .await
            .is_err()
    );
    release.send(()).unwrap();
    assert_eq!(count(&backend).await, 0);
    // Even a surviving Arc must not let the cancelled adapter operate again.
    assert!(tx
        .execute(query("INSERT INTO test VALUES (2)").unwrap())
        .await
        .is_err());
}

#[tokio::test]
async fn cancelled_acquisition_does_not_lose_capacity() {
    let backend = database().await;
    let held = backend.acquire().await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(10), backend.acquire())
            .await
            .is_err()
    );
    drop(held);
    assert_eq!(count(&backend).await, 0);
}

#[tokio::test]
async fn in_memory_connection_is_never_silently_replaced() {
    let backend = database().await;
    let object = backend.inner.pool().get().await.unwrap();
    drop(Object::take(object));
    assert!(backend.acquire().await.is_err());
    assert!(backend.inner.pool().is_closed());
    assert!(backend.acquire().await.is_err());
}

#[tokio::test]
async fn in_memory_poisoned_connection_fails_closed() {
    #[cfg(feature = "prometheus")]
    let failures = cleanup_count("worker_error");
    let backend = database().await;
    let conn = backend.acquire().await.unwrap();
    assert!(conn
        .run::<(), _>(|_| panic!("controlled worker failure"))
        .await
        .is_err());
    drop(conn);
    assert!(backend.acquire().await.is_err());
    assert!(backend.inner.pool().is_closed());
    #[cfg(feature = "prometheus")]
    assert!(cleanup_count("worker_error") >= failures + 1.0);
}

#[tokio::test]
async fn cleanup_deadline_discards_memory_connection_with_unfinished_work() {
    #[cfg(feature = "prometheus")]
    let timeouts = cleanup_count("timeout");
    let backend = database().await;
    let tx = backend.begin_transaction().await.unwrap();
    let (started, entered) = oneshot::channel();
    let (release, blocked) = mpsc::channel();
    let task = tokio::spawn(async move {
        tx.0.run(move |_| {
            started.send(()).unwrap();
            blocked.recv().unwrap();
            Ok(())
        })
        .await
    });
    entered.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::pause();
    // Let the rollback task start and register its deadline.
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(11)).await;
    tokio::task::yield_now().await;
    let closed = backend.inner.pool().is_closed();
    release.send(()).unwrap();
    assert!(closed, "cleanup timeout must close an in-memory backend");
    #[cfg(feature = "prometheus")]
    assert_eq!(cleanup_count("timeout"), timeouts + 1.0);
}

#[test]
fn transaction_and_backend_can_drop_after_runtime_shutdown() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (backend, tx) = runtime.block_on(async {
        let backend = database().await;
        let tx = backend.begin_transaction().await.unwrap();
        (backend, tx)
    });
    drop(runtime);
    drop(tx);
    assert!(backend.inner.pool().is_closed());
    drop(backend);
}

#[tokio::test]
async fn busy_file_transaction_does_not_block_tokio_and_recovers() {
    let path = std::env::temp_dir().join(format!("cdk-deadpool-{}.sqlite", uuid::Uuid::new_v4()));
    let backend = SqliteBackend::new(Config::from(&path)).unwrap();
    let first = backend.begin_transaction().await.unwrap();
    let second = backend.acquire().await.unwrap();
    let (started, entered) = oneshot::channel();
    let pending = tokio::spawn(async move {
        second
            .run(move |conn| {
                started.send(()).unwrap();
                conn.execute_batch("BEGIN IMMEDIATE")
                    .map_err(database_error)
            })
            .await
            .unwrap();
        second
            .run(|conn| conn.execute_batch("ROLLBACK").map_err(database_error))
            .await
            .unwrap();
    });
    entered.await.unwrap();
    // A current-thread runtime can get here while the worker waits on SQLite.
    first.rollback().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), pending)
        .await
        .unwrap()
        .unwrap();
    drop(backend);
    std::fs::remove_file(path).unwrap();
}

// Occupy the actual blocking worker so transaction-control cancellation happens
// after submission but before the command can execute.
async fn block_worker(conn: &SqliteConnection) -> mpsc::Sender<()> {
    use std::future::Future;
    use std::task::Poll;

    let (started, entered) = oneshot::channel();
    let (release, blocked) = mpsc::channel();
    {
        let slot = conn.lease.lock().await;
        let mut work = Box::pin(slot.as_ref().unwrap().object().interact(move |_| {
            started.send(()).unwrap();
            blocked.recv().unwrap();
        }));
        std::future::poll_fn(|cx| {
            assert!(work.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        // The future is gone; its blocking closure still owns the driver lock.
    }
    entered.await.unwrap();
    release
}

async fn cancel_after_submission<F>(future: F)
where
    F: std::future::Future,
{
    let mut future = Box::pin(future);
    std::future::poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
}

#[tokio::test]
async fn cancelled_begin_commit_and_rollback_finish_before_reuse() {
    let backend = database().await;
    let conn = backend.acquire().await.unwrap();
    let release = block_worker(&conn).await;
    cancel_after_submission(conn.begin_transaction()).await;
    release.send(()).unwrap();
    assert_eq!(count(&backend).await, 0);
    for commit in [false, true] {
        let tx = backend.begin_transaction().await.unwrap();
        tx.execute(query("INSERT INTO test VALUES (1)").unwrap())
            .await
            .unwrap();
        let release = block_worker(&tx.0).await;
        if commit {
            cancel_after_submission(tx.commit()).await;
        } else {
            cancel_after_submission(tx.rollback()).await;
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(10), backend.acquire())
                .await
                .is_err()
        );
        release.send(()).unwrap();
        assert_eq!(count(&backend).await, i64::from(commit));
    }
}

#[test]
fn cancelled_cleanup_on_runtime_shutdown_discards_connection() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let backend = runtime.block_on(async {
        let backend = database().await;
        drop(backend.begin_transaction().await.unwrap());
        // The rollback task is queued but has not run on this current thread.
        backend
    });
    drop(runtime);
    assert!(backend.inner.pool().is_closed());
    drop(backend);
}

#[tokio::test]
async fn file_database_recovers_after_worker_failure() {
    let path = std::env::temp_dir().join(format!("cdk-deadpool-{}.sqlite", uuid::Uuid::new_v4()));
    let backend = SqliteBackend::new(Config::from(&path)).unwrap();
    let conn = backend.acquire().await.unwrap();
    conn.batch(query("CREATE TABLE test (id INTEGER); INSERT INTO test VALUES (1)").unwrap())
        .await
        .unwrap();
    assert!(conn
        .run::<(), _>(|_| panic!("controlled worker failure"))
        .await
        .is_err());
    drop(conn);
    assert_eq!(count(&backend).await, 1);
    drop(backend);
    std::fs::remove_file(path).unwrap();
}

#[cfg(feature = "wallet")]
#[tokio::test]
async fn derivation_reservations_are_atomic_across_independent_pools() {
    use cdk_common::database::WalletDatabase;

    let path = std::env::temp_dir().join(format!("cdk-counter-{}.sqlite", uuid::Uuid::new_v4()));
    let first = crate::WalletSqliteDatabase::new(&path).await.unwrap();
    let second = crate::WalletSqliteDatabase::new(&path).await.unwrap();
    let (a, b, c, d) = tokio::join!(
        first.reserve_derivation_index("p2pk", 6),
        second.reserve_derivation_index("p2pk", 6),
        first.reserve_derivation_index("p2pk", 6),
        second.reserve_derivation_index("p2pk", 6),
    );
    let mut indexes = [a, b, c, d].map(Result::unwrap);
    indexes.sort_unstable();
    assert_eq!(indexes, [6, 7, 8, 9]);
    drop(first);
    drop(second);
    std::fs::remove_file(path).unwrap();
}

#[cfg(feature = "prometheus")]
fn cleanup_count(reason: &str) -> f64 {
    cdk_prometheus::METRICS
        .registry()
        .gather()
        .iter()
        .filter(|family| family.name() == "cdk_db_connections_discarded_total")
        .flat_map(|family| family.get_metric())
        .filter(|metric| {
            metric
                .get_label()
                .iter()
                .any(|label| label.name() == "reason" && label.value() == reason)
        })
        .map(|metric| metric.get_counter().value())
        .sum()
}

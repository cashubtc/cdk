use std::sync::atomic::Ordering;
use std::time::Duration;

use cdk_sql_common::pool;
use cdk_sql_common::stmt::query;
use tokio::time::timeout;

use super::{Fixture, Phase, Release};

#[tokio::test(flavor = "current_thread")]
async fn close_rejects_checkouts_and_can_resume_after_cancellation() {
    let fixture = Fixture::new().await;
    let clone = fixture.pool.clone();
    let connection = fixture.pool.get().await.unwrap();
    let mut waiting = Box::pin(clone.get());
    assert!(timeout(Duration::from_millis(20), &mut waiting)
        .await
        .is_err());

    let mut closing = Box::pin(fixture.pool.close());
    assert!(timeout(Duration::from_millis(20), &mut closing)
        .await
        .is_err());
    assert!(matches!(waiting.await, Err(pool::Error::Closed)));
    assert!(matches!(clone.get().await, Err(pool::Error::Closed)));
    drop(closing);

    // Already-borrowed connections must still be able to finish their work.
    query("INSERT INTO data VALUES (1)")
        .unwrap()
        .execute(&*connection)
        .await
        .unwrap();
    drop(connection);
    timeout(Duration::from_secs(2), clone.close())
        .await
        .unwrap()
        .unwrap();
    fixture.pool.close().await.unwrap();
    assert!(matches!(clone.get().await, Err(pool::Error::Closed)));
}

#[tokio::test(flavor = "current_thread")]
async fn close_waits_for_sql_detached_by_cancellation() {
    let fixture = Fixture::new().await;
    let mut issued = fixture
        .config
        .statements
        .arm("INSERT INTO data VALUES (1)", Phase::Before);
    let pool = fixture.pool.clone();
    let writer = tokio::spawn(async move {
        let connection = pool.get().await.unwrap();
        query("INSERT INTO data VALUES (1)")
            .unwrap()
            .execute(&*connection)
            .await
    });
    issued.entered().await;
    writer.abort();
    assert!(writer.await.unwrap_err().is_cancelled());

    let mut closing = Box::pin(fixture.pool.close());
    assert!(timeout(Duration::from_millis(20), &mut closing)
        .await
        .is_err());
    issued.release(Release::Continue);
    timeout(Duration::from_secs(2), closing)
        .await
        .unwrap()
        .unwrap();

    let connection = rusqlite::Connection::open(&fixture.config.path).unwrap();
    let value: i64 = connection
        .query_row("SELECT id FROM data", [], |row| row.get(0))
        .unwrap();
    assert_eq!(value, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn close_waits_for_cancelled_connection_creation() {
    let fixture = Fixture::unopened();
    let mut opening = fixture.config.constructor.arm("open", Phase::Before);
    let pool = fixture.pool.clone();
    let constructor = tokio::spawn(async move { pool.get().await });
    opening.entered().await;
    constructor.abort();
    assert!(constructor.await.unwrap_err().is_cancelled());

    let mut closing = Box::pin(fixture.pool.close());
    assert!(timeout(Duration::from_millis(20), &mut closing)
        .await
        .is_err());
    opening.release(Release::Continue);
    timeout(Duration::from_secs(2), closing)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fixture.config.creates.load(Ordering::SeqCst), 1);
    assert!(matches!(fixture.pool.get().await, Err(pool::Error::Closed)));
}

#[tokio::test(flavor = "current_thread")]
async fn stale_disposal_yields_and_close_waits_for_it() {
    let fixture = Fixture::new().await;
    let transaction = fixture.transaction().await;
    query("INSERT INTO data VALUES (1)")
        .unwrap()
        .execute(&transaction)
        .await
        .unwrap();
    let mut rollback = fixture.config.statements.arm("ROLLBACK", Phase::Before);
    drop(transaction);
    rollback.entered().await;
    let mut disposal = fixture.config.statements.arm("close", Phase::Before);
    rollback.release(Release::Fail);
    // A synchronous close would block this runtime waiting for the release below.
    disposal.entered().await;

    let mut closing = Box::pin(fixture.pool.close());
    assert!(timeout(Duration::from_millis(20), &mut closing)
        .await
        .is_err());
    disposal.release(Release::Continue);
    timeout(Duration::from_secs(2), closing)
        .await
        .unwrap()
        .unwrap();
    let connection = rusqlite::Connection::open(&fixture.config.path).unwrap();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM data", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0, "closing must roll back the abandoned transaction");
}

#[tokio::test(flavor = "current_thread")]
async fn dropping_pool_dispatches_closure_off_async_workers() {
    let Fixture {
        pool,
        config,
        _directory,
    } = Fixture::new().await;
    let mut disposal = config.statements.arm("close", Phase::Before);
    drop(pool);
    disposal.entered().await;
    disposal.release(Release::Continue);
}

#[tokio::test(flavor = "current_thread")]
async fn checkout_disposes_idle_stale_connections_off_async_workers() {
    let fixture = Fixture::new().await;
    let connection = fixture.pool.get().await.unwrap();
    let stale = connection.stale.clone();
    drop(connection);
    // A detached operation can mark the connection stale after it was returned.
    stale.store(true, Ordering::SeqCst);
    let mut disposal = fixture.config.statements.arm("close", Phase::Before);
    let pool = fixture.pool.clone();
    let checkout = tokio::spawn(async move { pool.get().await });
    disposal.entered().await;

    let mut closing = Box::pin(fixture.pool.close());
    assert!(timeout(Duration::from_millis(20), &mut closing)
        .await
        .is_err());
    disposal.release(Release::Continue);
    let connection = checkout.await.unwrap().unwrap();
    assert_eq!(fixture.config.creates.load(Ordering::SeqCst), 2);
    drop(connection);
    timeout(Duration::from_secs(2), closing)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_close_keeps_disposal_tracked_for_another_caller() {
    let fixture = Fixture::new().await;
    let mut disposal = fixture.config.statements.arm("close", Phase::Before);
    let pool = fixture.pool.clone();
    let closer = tokio::spawn(async move { pool.close().await });
    disposal.entered().await;
    closer.abort();
    assert!(closer.await.unwrap_err().is_cancelled());

    let mut retry = Box::pin(fixture.pool.close());
    assert!(timeout(Duration::from_millis(20), &mut retry)
        .await
        .is_err());
    disposal.release(Release::Continue);
    timeout(Duration::from_secs(2), retry)
        .await
        .unwrap()
        .unwrap();
}

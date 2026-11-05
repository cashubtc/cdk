use std::sync::atomic::Ordering;
use std::time::Duration;

use cdk_sql_common::stmt::query;
use rusqlite::Connection;

use super::{Fixture, Phase, Release, Transaction};

#[tokio::test(flavor = "current_thread")]
async fn cancelled_begin_retains_loan_until_orphan_transaction_is_cleaned() {
    let fixture = Fixture::new().await;
    let holder = Connection::open(&fixture.config.path).unwrap();
    holder.execute_batch("BEGIN IMMEDIATE").unwrap();
    let mut entered = fixture
        .config
        .statements
        .arm("BEGIN IMMEDIATE", Phase::Before);
    let pool = fixture.pool.clone();
    let worker = tokio::spawn(async move { Transaction::new(pool.get().await.unwrap()).await });
    entered.entered().await;
    entered.release(Release::Continue);
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());

    // This timeout is also a current-thread timer-progress assertion while the
    // real SQLite BEGIN is contending with the independent writer.
    fixture.assert_loan_held().await;
    holder.execute_batch("COMMIT").unwrap();
    fixture.assert_clean_and_write(vec![]).await;
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_commit_waits_for_sql_outcome_before_releasing_loan() {
    let fixture = Fixture::new().await;
    let transaction = fixture.transaction().await;
    query("INSERT INTO data VALUES (1), (2)")
        .unwrap()
        .execute(&transaction)
        .await
        .unwrap();
    let mut finished = fixture.config.statements.arm("COMMIT", Phase::After);
    let worker = tokio::spawn(transaction.commit());
    finished.entered().await;
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    fixture.assert_loan_held().await;
    finished.release(Release::Continue);
    // Both writes really committed before cancellation. A lost acknowledgement
    // cannot turn that atomic outcome into a partial or duplicated transaction.
    fixture.assert_clean_and_write(vec![1, 2]).await;
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_rollback_waits_for_outcome_and_cannot_touch_next_borrower() {
    let fixture = Fixture::new().await;
    let transaction = fixture.transaction().await;
    query("INSERT INTO data VALUES (1), (2)")
        .unwrap()
        .execute(&transaction)
        .await
        .unwrap();
    let mut finished = fixture.config.statements.arm("ROLLBACK", Phase::After);
    let worker = tokio::spawn(transaction.rollback());
    finished.entered().await;
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    fixture.assert_loan_held().await;
    finished.release(Release::Continue);
    fixture.assert_clean_and_write(vec![]).await;
}

#[tokio::test(flavor = "current_thread")]
async fn dropped_transaction_holds_loan_through_cleanup() {
    let fixture = Fixture::new().await;
    let transaction = fixture.transaction().await;
    query("INSERT INTO data VALUES (1)")
        .unwrap()
        .execute(&transaction)
        .await
        .unwrap();
    let mut cleanup = fixture.config.statements.arm("ROLLBACK", Phase::Before);
    drop(transaction);
    cleanup.entered().await;
    fixture.assert_loan_held().await;
    cleanup.release(Release::Continue);
    fixture.assert_clean_and_write(vec![]).await;
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_ordinary_query_cannot_be_overtaken_by_new_transaction() {
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
    let connection = fixture.pool.get().await.unwrap();
    let mut next = tokio::spawn(async move { Transaction::new(connection).await });
    assert!(tokio::time::timeout(Duration::from_millis(20), &mut next)
        .await
        .is_err());
    assert_eq!(fixture.config.creates.load(Ordering::SeqCst), 1);
    issued.release(Release::Continue);
    let transaction = next.await.unwrap().unwrap();
    let rows = query("SELECT id FROM data")
        .unwrap()
        .fetch_all(&transaction)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "cancelled SQL must precede the next BEGIN");
    transaction.rollback().await.unwrap();
    fixture.assert_clean_and_write(vec![1]).await;
}

#[tokio::test(flavor = "current_thread")]
async fn failed_rollback_discards_open_transaction() {
    let fixture = Fixture::new().await;
    let transaction = fixture.transaction().await;
    query("INSERT INTO data VALUES (1)")
        .unwrap()
        .execute(&transaction)
        .await
        .unwrap();
    let mut cleanup = fixture.config.statements.arm("ROLLBACK", Phase::Before);
    drop(transaction);
    cleanup.entered().await;
    fixture.assert_loan_held().await;
    cleanup.release(Release::Fail);
    fixture.assert_clean_and_write(vec![]).await;
    assert_eq!(fixture.config.creates.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn panicked_statement_discards_connection_and_rolls_back_atomically() {
    let fixture = Fixture::new().await;
    let transaction = fixture.transaction().await;
    let mut panic = fixture
        .config
        .statements
        .arm("INSERT INTO data VALUES (1), (2)", Phase::After);
    let worker = tokio::spawn(async move {
        let result = query("INSERT INTO data VALUES (1), (2)")
            .unwrap()
            .execute(&transaction)
            .await;
        drop(transaction);
        result
    });
    panic.entered().await;
    panic.release(Release::Panic);
    assert!(worker.await.unwrap().is_err());
    fixture.assert_clean_and_write(vec![]).await;
    assert_eq!(fixture.config.creates.load(Ordering::SeqCst), 2);
}

#[test]
fn cancelled_cleanup_on_runtime_shutdown_never_reuses_abandoned_transaction() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (fixture, cleanup) = runtime.block_on(async {
        let fixture = Fixture::new().await;
        let transaction = fixture.transaction().await;
        query("INSERT INTO data VALUES (1)")
            .unwrap()
            .execute(&transaction)
            .await
            .unwrap();
        let mut cleanup = fixture.config.statements.arm("ROLLBACK", Phase::Before);
        drop(transaction);
        cleanup.entered().await;
        (fixture, cleanup)
    });
    runtime.shutdown_background();
    cleanup.release(Release::Fail);
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            fixture.assert_clean_and_write(vec![]).await;
            assert_eq!(fixture.config.creates.load(Ordering::SeqCst), 2);
        });
}

#[tokio::test(flavor = "current_thread")]
async fn successful_drop_rollback_preserves_in_memory_database() {
    use cdk_sql_common::database::ConnectionWithTransaction;
    use cdk_sql_common::pool::Pool;

    use crate::common::{Config, SqliteConnectionManager};

    let pool = Pool::<SqliteConnectionManager>::new(Config::from(":memory:"));
    let connection = pool.get().await.unwrap();
    query("CREATE TABLE data (id INTEGER PRIMARY KEY); INSERT INTO data VALUES (1)")
        .unwrap()
        .batch(&*connection)
        .await
        .unwrap();
    drop(connection);
    let transaction = ConnectionWithTransaction::new(pool.get().await.unwrap())
        .await
        .unwrap();
    query("INSERT INTO data VALUES (2)")
        .unwrap()
        .execute(&transaction)
        .await
        .unwrap();
    drop(transaction);
    let connection = pool.get().await.unwrap();
    let rows = query("SELECT id FROM data")
        .unwrap()
        .fetch_all(&*connection)
        .await
        .unwrap();
    assert_eq!(rows, vec![vec![cdk_sql_common::value::Value::Integer(1)]]);
}

#[test]
fn unpolled_cleanup_is_fenced_before_runtime_discards_it() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let fixture = runtime.block_on(async {
        let fixture = Fixture::new().await;
        let transaction = fixture.transaction().await;
        query("INSERT INTO data VALUES (1)")
            .unwrap()
            .execute(&transaction)
            .await
            .unwrap();
        // There is no yield after spawning cleanup, so this current-thread
        // runtime returns before the cleanup future can be polled.
        drop(transaction);
        fixture
    });
    runtime.shutdown_background();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            fixture.assert_clean_and_write(vec![]).await;
            assert_eq!(fixture.config.creates.load(Ordering::SeqCst), 2);
        });
}

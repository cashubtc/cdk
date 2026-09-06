//! Connection metrics are process-global; keep this test in its own test binary.
#![cfg(feature = "prometheus")]

use std::time::Duration;

use cdk_prometheus::METRICS;
use cdk_sql_common::database::{DatabaseExecutor, SqlBackend, SqlConnection, SqlTransaction};
use cdk_sql_common::stmt::query;

fn active() -> f64 {
    METRICS
        .registry()
        .gather()
        .iter()
        .find(|family| family.name() == "cdk_db_connections_active")
        .expect("connection gauge is registered")
        .get_metric()[0]
        .get_gauge()
        .value()
}

#[tokio::test]
async fn checkouts_errors_and_cleanup_balance_gauge() {
    let backend = cdk_sqlite::SqliteBackend::new(cdk_sqlite::Config::from(":memory:")).unwrap();
    assert_eq!(active(), 0.0);
    let conn = backend.acquire().await.unwrap();
    assert_eq!(active(), 1.0);
    assert!(
        tokio::time::timeout(Duration::from_millis(10), backend.acquire())
            .await
            .is_err()
    );
    assert_eq!(
        active(),
        1.0,
        "cancelled checkout must not increment the gauge"
    );
    let tx = conn.begin_transaction().await.unwrap();
    assert_eq!(
        active(),
        1.0,
        "begin must not count the same checkout twice"
    );
    tx.commit().await.unwrap();
    assert_eq!(active(), 0.0);
    let tx = backend.begin_transaction().await.unwrap();
    assert!(tx
        .execute(query("INSERT INTO definitely_missing_table VALUES (1)").unwrap())
        .await
        .is_err());
    drop(tx);
    tokio::time::timeout(Duration::from_secs(5), async {
        while active() != 0.0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let tx = backend.begin_transaction().await.unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(active(), 0.0);
    let missing = std::env::temp_dir()
        .join(format!("cdk-missing-{}", uuid::Uuid::new_v4()))
        .join("test.sqlite");
    let invalid = cdk_sqlite::SqliteBackend::new(cdk_sqlite::Config::from(missing)).unwrap();
    assert!(invalid.acquire().await.is_err());
    assert_eq!(
        active(),
        0.0,
        "failed checkout must not increment the gauge"
    );
}

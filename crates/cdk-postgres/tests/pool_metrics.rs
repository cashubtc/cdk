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
    let url = std::env::var("CDK_MINTD_DATABASE_URL")
        .or_else(|_| std::env::var("PG_DB_URL"))
        .unwrap_or_else(|_| {
            "host=localhost user=cdk_user password=cdk_password dbname=cdk_mint port=5432"
                .to_owned()
        });
    let backend =
        cdk_postgres::PostgresBackend::new(cdk_postgres::PgConfig::new(&url, None, Some(1), None))
            .unwrap();
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
    let invalid = cdk_postgres::PostgresBackend::new(cdk_postgres::PgConfig::new(
        "host=127.0.0.1 port=1 user=invalid dbname=invalid",
        None,
        Some(1),
        Some(1),
    ))
    .unwrap();
    assert!(invalid.acquire().await.is_err());
    assert_eq!(
        active(),
        0.0,
        "failed checkout must not increment the gauge"
    );
}

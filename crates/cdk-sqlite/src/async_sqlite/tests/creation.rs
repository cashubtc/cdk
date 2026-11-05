use std::sync::atomic::Ordering;

use cdk_sql_common::pool;
use cdk_sql_common::stmt::query;

use super::{Fixture, Phase, Release};

#[tokio::test(flavor = "current_thread")]
async fn cancelled_constructor_keeps_capacity_until_blocking_work_finishes() {
    let fixture = Fixture::unopened();
    let mut opening = fixture.config.constructor.arm("open", Phase::Before);
    let pool = fixture.pool.clone();
    let worker = tokio::spawn(async move { pool.get().await });
    opening.entered().await;
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    fixture.assert_loan_held().await;
    opening.release(Release::Continue);
    let connection = fixture.pool.get().await.unwrap();
    query("CREATE TABLE data (id INTEGER PRIMARY KEY)")
        .unwrap()
        .execute(&*connection)
        .await
        .unwrap();
    assert_eq!(fixture.config.creates.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn constructor_error_and_panic_return_capacity_exactly_once() {
    for action in [Release::Fail, Release::Panic] {
        let fixture = Fixture::unopened();
        let mut opening = fixture.config.constructor.arm("open", Phase::Before);
        let pool = fixture.pool.clone();
        let worker = tokio::spawn(async move { pool.get().await });
        opening.entered().await;
        fixture.assert_loan_held().await;
        opening.release(action);
        let result = worker.await.unwrap();
        assert!(
            matches!(result, Err(pool::Error::Poison | pool::Error::Task(_))),
            "{result:?}"
        );
        let connection = fixture.pool.get().await.unwrap();
        assert_eq!(fixture.config.creates.load(Ordering::SeqCst), 2);
        assert!(matches!(
            fixture
                .pool
                .get_timeout(std::time::Duration::from_millis(20))
                .await,
            Err(pool::Error::Timeout)
        ));
        query("SELECT 1")
            .unwrap()
            .fetch_one(&*connection)
            .await
            .unwrap();
    }
}

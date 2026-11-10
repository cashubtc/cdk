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

fn without_runtime<F>(future: F) -> F::Output
where
    F: std::future::Future,
{
    assert!(tokio::runtime::Handle::try_current().is_err());
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    match std::pin::pin!(future).poll(&mut context) {
        std::task::Poll::Ready(result) => result,
        std::task::Poll::Pending => panic!("operation without a runtime should fail immediately"),
    }
}

#[test]
fn pool_without_runtime_returns_error_before_constructing_or_waiting() {
    let fixture = Fixture::unopened();
    assert!(matches!(
        without_runtime(fixture.pool.get()),
        Err(pool::Error::MissingRuntime)
    ));
    assert!(matches!(
        without_runtime(fixture.pool.close()),
        Err(pool::Error::MissingRuntime)
    ));
    assert_eq!(fixture.config.creates.load(Ordering::SeqCst), 0);

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _connection = runtime.block_on(fixture.pool.get()).unwrap();
    // The exhausted-pool path must not reach Tokio's timer without a runtime.
    assert!(matches!(
        without_runtime(fixture.pool.get()),
        Err(pool::Error::MissingRuntime)
    ));
    assert_eq!(fixture.config.creates.load(Ordering::SeqCst), 1);
}

#[test]
fn statement_without_runtime_returns_error_and_keeps_connection_usable() {
    use cdk_sql_common::database::DatabaseExecutor;

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let fixture = runtime.block_on(Fixture::new());
    let connection = runtime.block_on(fixture.pool.get()).unwrap();
    let result = without_runtime(connection.pluck(query("SELECT 1").unwrap()));
    assert!(
        matches!(result, Err(cdk_common::database::Error::Internal(message))
        if message == "SQLite operations require a Tokio runtime")
    );
    assert_eq!(
        runtime
            .block_on(connection.pluck(query("SELECT 1").unwrap()))
            .unwrap(),
        Some(cdk_sql_common::value::Value::Integer(1))
    );
}

#[test]
fn blocking_statement_enters_the_callers_tracing_span() {
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    use tracing_subscriber::prelude::*;

    struct ObserveWorkerSpan {
        caller: std::thread::ThreadId,
        entered: Arc<AtomicBool>,
    }

    impl<S> tracing_subscriber::Layer<S> for ObserveWorkerSpan
    where
        S: tracing::Subscriber,
    {
        fn on_enter(&self, _: &tracing::span::Id, _: tracing_subscriber::layer::Context<'_, S>) {
            if std::thread::current().id() != self.caller {
                self.entered.store(true, Ordering::SeqCst);
            }
        }
    }

    let entered = Arc::new(AtomicBool::new(false));
    let subscriber = tracing_subscriber::registry().with(ObserveWorkerSpan {
        caller: std::thread::current().id(),
        entered: entered.clone(),
    });
    tracing::subscriber::with_default(subscriber, || {
        let span = tracing::info_span!("sqlite_caller");
        let _entered = span.enter();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let fixture = runtime.block_on(Fixture::new());
        let connection = runtime.block_on(fixture.pool.get()).unwrap();
        entered.store(false, Ordering::SeqCst);
        runtime
            .block_on(query("SELECT 1").unwrap().pluck(&*connection))
            .unwrap();
        assert!(
            entered.load(Ordering::SeqCst),
            "blocking statement lost its caller's span"
        );
    });
}

//! Shared cross-instance test suite for the SQL polling pub/sub bus.
//!
//! Gated by `feature = "test"` (which also pulls in `cdk-common/test` for the
//! generic pub/sub types). A backend crate supplies an
//! `async fn(String) -> Arc<Pool<RM>>` that returns one per-test-isolated store
//! shared across connections, then invokes [`crate::sql_bus_test!`] to generate
//! the `#[tokio::test]` wrappers. The suite builds two logical instances on that
//! store (two [`SqlBusConnector::connect`] calls produce two distinct origins),
//! which is all the cross-instance behavior depends on.
#![allow(clippy::unwrap_used, clippy::missing_panics_doc)]

use std::sync::Arc;
use std::time::Duration;

use cdk_common::pub_sub::test::{CustomPubSub, Message, SubscriptionReq};
use cdk_common::pub_sub::{Pubsub, Spec};
use tokio::time::timeout;

use crate::mint::bus::{SqlBusConnector, SqlBusOptions};
use crate::pool::{DatabasePool, Pool};
use crate::stmt::query;
use crate::value::Value;

/// Short poll interval so the poll-driven assertions resolve quickly.
fn fast_options() -> SqlBusOptions {
    SqlBusOptions {
        poll_interval: Duration::from_millis(25),
        retention: Duration::from_secs(3600),
    }
}

/// Build one logical instance (its own origin) over a shared pool.
async fn instance<RM: DatabasePool + 'static>(pool: Arc<Pool<RM>>) -> Pubsub<CustomPubSub> {
    let connector = SqlBusConnector::connect(pool, fast_options())
        .await
        .expect("connect sql bus");
    Pubsub::new_with_bus(CustomPubSub::new_instance(()), move |local| {
        connector.build(local)
    })
}

/// Current number of rows in the outbox.
async fn outbox_len<RM: DatabasePool + 'static>(pool: &Arc<Pool<RM>>) -> i64 {
    let conn = pool.get().await.expect("connection");
    match query("SELECT COUNT(*) FROM pubsub_outbox")
        .expect("statement")
        .pluck(&*conn)
        .await
        .expect("pluck")
    {
        Some(Value::Integer(n)) => n,
        _ => 0,
    }
}

/// Wait until the outbox holds at least `want` rows.
///
/// `publish` appends its row from a spawned task, so a test that must observe an
/// event as durably stored (before connecting a later instance) polls for it
/// rather than sleeping a fixed amount.
async fn wait_for_rows<RM: DatabasePool + 'static>(pool: &Arc<Pool<RM>>, want: i64) {
    for _ in 0..200 {
        if outbox_len(pool).await >= want {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("outbox never reached {want} rows");
}

/// Publish on A: B receives it through the outbox, A receives it once locally,
/// and A's own row is filtered out by origin so it is never echoed back.
pub async fn event_crosses_instances<RM: DatabasePool + 'static>(pool: Arc<Pool<RM>>) {
    let a = instance(pool.clone()).await;
    let b = instance(pool.clone()).await;

    let mut sub_a = a.subscribe(SubscriptionReq::Foo(2)).unwrap();
    let mut sub_b = b.subscribe(SubscriptionReq::Foo(2)).unwrap();

    a.publish(Message { foo: 2, bar: 7 });

    let received_b = timeout(Duration::from_secs(5), sub_b.recv())
        .await
        .expect("event delivered to instance B before timeout");
    assert_eq!(received_b.map(|m| m.bar), Some(7));

    let received_a = timeout(Duration::from_secs(5), sub_a.recv())
        .await
        .expect("event delivered to instance A before timeout");
    assert_eq!(received_a.map(|m| m.bar), Some(7));

    // A's own outbox row is filtered by origin, so A sees the event once.
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(sub_a.try_recv().is_none());
}

/// An instance starts from the current outbox high-water mark: events published
/// before it connects are not replayed, only later ones are delivered.
pub async fn instance_starts_from_now_ignoring_history<RM: DatabasePool + 'static>(
    pool: Arc<Pool<RM>>,
) {
    let a = instance(pool.clone()).await;
    a.publish(Message { foo: 2, bar: 1 }); // E1, before B exists

    // Ensure E1 is durably in the outbox before B connects, so B's starting
    // cursor sits past it.
    wait_for_rows(&pool, 1).await;

    let b = instance(pool.clone()).await;
    let mut sub_b = b.subscribe(SubscriptionReq::Foo(2)).unwrap();

    a.publish(Message { foo: 2, bar: 2 }); // E2, after B connected

    let received = timeout(Duration::from_secs(5), sub_b.recv())
        .await
        .expect("E2 delivered to instance B before timeout");
    // B sees E2 and never the pre-connect E1.
    assert_eq!(received.map(|m| m.bar), Some(2));
    assert!(sub_b.try_recv().is_none());
}

/// Several events published on A all reach B.
///
/// Asserted as a set: the bus documents best-effort id ordering under concurrent
/// writers, so delivery order is not guaranteed.
pub async fn multiple_events_delivered<RM: DatabasePool + 'static>(pool: Arc<Pool<RM>>) {
    let a = instance(pool.clone()).await;
    let b = instance(pool.clone()).await;

    let mut sub_b = b.subscribe(SubscriptionReq::Foo(2)).unwrap();

    for bar in [10u64, 11, 12] {
        a.publish(Message { foo: 2, bar });
    }

    let mut received = Vec::new();
    for _ in 0..3 {
        let event = timeout(Duration::from_secs(5), sub_b.recv())
            .await
            .expect("event delivered to instance B before timeout");
        received.push(event.expect("event present").bar);
    }
    received.sort_unstable();
    assert_eq!(received, vec![10, 11, 12]);
}

/// Generate `#[tokio::test]` wrappers running the shared SQL bus suite against a
/// backend-provided store factory.
///
/// `$make_fn` is an `async fn(String) -> Arc<Pool<RM>>` returning one
/// per-test-isolated store shared across connections. Mirrors
/// [`cdk_common::bus_test!`].
#[macro_export]
macro_rules! sql_bus_test {
    ($make_fn:ident) => {
        $crate::sql_bus_test!(
            $make_fn,
            event_crosses_instances,
            instance_starts_from_now_ignoring_history,
            multiple_events_delivered,
        );
    };
    ($make_fn:ident, $($name:ident),+ $(,)?) => {
        $(
            #[tokio::test]
            async fn $name() {
                use std::time::{SystemTime, UNIX_EPOCH};
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("Time went backwards");
                let pool = $make_fn(format!("test_{}_{}", now.as_nanos(), stringify!($name))).await;
                $crate::mint::bus_test::$name(pool).await;
            }
        )+
    };
}

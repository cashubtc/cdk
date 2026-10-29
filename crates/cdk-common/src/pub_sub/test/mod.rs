//! Shared pub/sub test suite
//!
//! The generic bodies here exercise the [`Bus`](super::Bus) contract through a
//! [`Pubsub`](super::Pubsub): an event published on a node reaches that node's
//! local subscribers, whatever bus is wired in. They are written once and run
//! against every backend, mirroring the generic database tests in
//! [`crate::database::mint::test`].
//!
//! A backend supplies a factory `async fn(String) -> Pubsub<CustomPubSub>` that
//! builds a `Pubsub` around the bus under test, then calls [`bus_test!`] to
//! emit one `#[tokio::test]` per body. `LocalBus` is exercised below;
//! `PostgresBus` is exercised from `cdk-postgres`.
//!
//! The module is gated by `#[cfg(any(test, feature = "test"))]` so cdk-common's
//! own `cargo test` sees it without the feature while dependent crates opt in
//! through `cdk-common/test`.
#![allow(clippy::unwrap_used, clippy::missing_panics_doc)]

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use super::{Error, Event, Pubsub, Spec, Subscriber, SubscriptionRequest};

/// Event used by the shared test suite.
#[derive(Clone, Debug, Serialize, Eq, PartialEq, Deserialize)]
pub struct Message {
    /// First topic key.
    pub foo: u64,
    /// Second topic key.
    pub bar: u64,
}

/// Topic index for [`Message`].
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Deserialize, Serialize)]
pub enum IndexTest {
    /// Matches [`Message::foo`].
    Foo(u64),
    /// Matches [`Message::bar`].
    Bar(u64),
}

impl Event for Message {
    type Topic = IndexTest;

    fn get_topics(&self) -> Vec<Self::Topic> {
        vec![IndexTest::Foo(self.foo), IndexTest::Bar(self.bar)]
    }
}

/// Minimal [`Spec`] backing the shared suite. Keeps the latest value per topic
/// in memory so `fetch_events` can backfill a new subscription.
#[derive(Debug)]
pub struct CustomPubSub {
    /// Latest value seen per topic.
    pub storage: Arc<RwLock<HashMap<IndexTest, Message>>>,
}

#[async_trait::async_trait]
impl Spec for CustomPubSub {
    type Topic = IndexTest;

    type Event = Message;

    type SubscriptionId = String;

    type Context = ();

    fn new_instance(_context: Self::Context) -> Arc<Self>
    where
        Self: Sized,
    {
        Arc::new(Self {
            storage: Default::default(),
        })
    }

    async fn fetch_events(
        self: &Arc<Self>,
        topics: Vec<<Self::Event as Event>::Topic>,
        reply_to: Subscriber<Self>,
    ) where
        Self: Sized,
    {
        let storage = self.storage.read().unwrap();

        for index in topics {
            if let Some(value) = storage.get(&index) {
                let _ = reply_to.send(value.clone());
            }
        }
    }
}

/// Subscription request over [`IndexTest`].
#[derive(Debug, Clone)]
pub enum SubscriptionReq {
    /// Subscribe to a [`IndexTest::Foo`] topic.
    Foo(u64),
    /// Subscribe to a [`IndexTest::Bar`] topic.
    Bar(u64),
}

impl SubscriptionRequest for SubscriptionReq {
    type Topic = IndexTest;

    type SubscriptionId = String;

    fn try_get_topics(&self) -> Result<Vec<Self::Topic>, Error> {
        Ok(vec![match self {
            SubscriptionReq::Bar(n) => IndexTest::Bar(*n),
            SubscriptionReq::Foo(n) => IndexTest::Foo(*n),
        }])
    }

    fn subscription_name(&self) -> Arc<Self::SubscriptionId> {
        Arc::new("test".to_owned())
    }
}

/// Subscription request whose topic resolution always fails, for the
/// error-path tests.
#[derive(Debug, Clone)]
pub struct FailingSubscriptionReq;

impl SubscriptionRequest for FailingSubscriptionReq {
    type Topic = IndexTest;

    type SubscriptionId = String;

    fn try_get_topics(&self) -> Result<Vec<Self::Topic>, Error> {
        Err(Error::ParsingError("intentional failure".to_string()))
    }

    fn subscription_name(&self) -> Arc<Self::SubscriptionId> {
        Arc::new("failing-sub".to_owned())
    }
}

// Generic bus bodies. Each takes a `Pubsub` already wired to the bus under
// test; the factory chose the bus. They assert only the single-node contract:
// publishing through the bus reaches local subscribers. Cross-instance fan-out
// is backend-specific and tested where it applies (see cdk-postgres).

/// A published event reaches a local subscriber on the matching topic.
pub async fn publish_reaches_local_subscriber(pubsub: Pubsub<CustomPubSub>) {
    let mut subscriber = pubsub.subscribe(SubscriptionReq::Foo(2)).unwrap();

    pubsub.publish(Message { foo: 2, bar: 7 });

    assert_eq!(subscriber.recv().await.map(|m| m.bar), Some(7));
}

/// A single published event fans out to every subscriber on the topic.
pub async fn publish_reaches_all_matching_subscribers(pubsub: Pubsub<CustomPubSub>) {
    let mut first = pubsub.subscribe(SubscriptionReq::Foo(2)).unwrap();
    let mut second = pubsub.subscribe(SubscriptionReq::Foo(2)).unwrap();

    pubsub.publish(Message { foo: 2, bar: 7 });

    assert_eq!(first.recv().await.map(|m| m.bar), Some(7));
    assert_eq!(second.recv().await.map(|m| m.bar), Some(7));
}

/// A subscriber only receives events on its own topic. Publishing a
/// non-matching event then a matching one, the subscriber sees only the
/// matching event, regardless of the order the two deliveries are scheduled.
pub async fn publish_skips_non_matching_topic(pubsub: Pubsub<CustomPubSub>) {
    let mut subscriber = pubsub.subscribe(SubscriptionReq::Foo(3)).unwrap();

    // Topics Foo(2)/Bar(1): does not match Foo(3).
    pubsub.publish(Message { foo: 2, bar: 1 });
    // Topics Foo(3)/Bar(2): matches.
    pubsub.publish(Message { foo: 3, bar: 2 });

    assert_eq!(subscriber.recv().await.map(|m| m.bar), Some(2));
    assert!(subscriber.try_recv().is_none());
}

/// Dropping the subscriber unregisters it, and publishing afterwards with no
/// subscribers is a no-op rather than a panic.
pub async fn dropped_subscriber_stops_delivery(pubsub: Pubsub<CustomPubSub>) {
    let subscriber = pubsub.subscribe(SubscriptionReq::Foo(2)).unwrap();
    assert_eq!(pubsub.active_subscribers(), 1);

    drop(subscriber);
    assert_eq!(pubsub.active_subscribers(), 0);

    pubsub.publish(Message { foo: 2, bar: 7 });
    // Let the (possibly spawned) delivery run; it must not panic.
    tokio::task::yield_now().await;
    assert_eq!(pubsub.active_subscribers(), 0);
}

/// Emit one `#[tokio::test]` per generic bus body for a backend.
///
/// `$make_fn` is an `async fn(String) -> Pubsub<CustomPubSub>` that builds a
/// `Pubsub` around the bus under test. The `String` is a unique per-test id a
/// stateful backend can use for isolation (for example a `LISTEN`/`NOTIFY`
/// channel name); a purely in-process bus ignores it.
#[macro_export]
macro_rules! bus_test {
    ($make_fn:ident) => {
        $crate::bus_test!(
            $make_fn,
            publish_reaches_local_subscriber,
            publish_reaches_all_matching_subscribers,
            publish_skips_non_matching_topic,
            dropped_subscriber_stops_delivery,
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

                $crate::pub_sub::test::$name(
                    $make_fn(format!("test_{}_{}", now.as_nanos(), stringify!($name))).await,
                )
                .await;
            }
        )+
    };
}

#[cfg(test)]
mod local_bus {
    use super::*;

    // In-process backend: the default bus is `LocalBus`, so `Pubsub::new`
    // already wires the bus under test. The id is unused (nothing to isolate).
    async fn provide_local_bus(_test_id: String) -> Pubsub<CustomPubSub> {
        Pubsub::new(CustomPubSub::new_instance(()))
    }

    crate::bus_test!(provide_local_bus);
}

#[cfg(test)]
mod unit {
    use super::*;

    #[tokio::test]
    async fn delivery_twice_realtime() {
        let pubsub = Pubsub::new(CustomPubSub::new_instance(()));

        assert_eq!(pubsub.active_subscribers(), 0);

        let mut subscriber = pubsub.subscribe(SubscriptionReq::Foo(2)).unwrap();

        assert_eq!(pubsub.active_subscribers(), 1);

        let _ = pubsub.publish_now(Message { foo: 2, bar: 1 });
        let _ = pubsub.publish_now(Message { foo: 2, bar: 2 });

        assert_eq!(subscriber.recv().await.map(|x| x.bar), Some(1));
        assert_eq!(subscriber.recv().await.map(|x| x.bar), Some(2));
        assert!(subscriber.try_recv().is_none());

        drop(subscriber);

        assert_eq!(pubsub.active_subscribers(), 0);
    }

    #[tokio::test]
    async fn failed_subscribe_does_not_leak_active_subscribers() {
        let pubsub = Pubsub::new(CustomPubSub::new_instance(()));

        assert_eq!(pubsub.active_subscribers(), 0);

        let result = pubsub.subscribe(FailingSubscriptionReq);

        assert!(result.is_err());
        assert_eq!(pubsub.active_subscribers(), 0);
    }

    #[tokio::test]
    async fn custom_bus_receives_published_events_and_still_delivers_locally() {
        use std::sync::Mutex;

        use crate::pub_sub::{Bus, LocalDelivery};

        // A bus that records every published event and forwards it to local
        // subscribers. Stands in for a real cross-process bus, whose forwarding
        // step would additionally hit the wire.
        struct RecordingBus {
            local: LocalDelivery<CustomPubSub>,
            seen: Arc<Mutex<Vec<Message>>>,
        }

        impl Bus<CustomPubSub> for RecordingBus {
            fn publish(&self, event: Message) {
                self.seen.lock().unwrap().push(event.clone());
                self.local.deliver(event);
            }
        }

        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_for_bus = seen.clone();
        let pubsub = Pubsub::new_with_bus(CustomPubSub::new_instance(()), move |local| {
            Arc::new(RecordingBus {
                local,
                seen: seen_for_bus,
            })
        });

        let mut subscriber = pubsub.subscribe(SubscriptionReq::Foo(2)).unwrap();

        pubsub.publish(Message { foo: 2, bar: 7 });

        // The event reached the local subscriber through the custom bus.
        assert_eq!(subscriber.recv().await.map(|x| x.bar), Some(7));
        // And the bus observed it on the way out.
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            &[Message { foo: 2, bar: 7 }]
        );
    }

    #[tokio::test]
    async fn read_from_storage() {
        let x = CustomPubSub::new_instance(());
        let storage = x.storage.clone();

        let pubsub = Pubsub::new(x);

        {
            // set previous value
            let mut s = storage.write().unwrap();
            s.insert(IndexTest::Bar(2), Message { foo: 3, bar: 2 });
        }

        let mut subscriber = pubsub.subscribe(SubscriptionReq::Bar(2)).unwrap();

        // Just should receive the latest
        assert_eq!(subscriber.recv().await.map(|x| x.foo), Some(3));

        // realtime delivery test
        let _ = pubsub.publish_now(Message { foo: 1, bar: 2 });
        assert_eq!(subscriber.recv().await.map(|x| x.foo), Some(1));

        {
            // set previous value
            let mut s = storage.write().unwrap();
            s.insert(IndexTest::Bar(2), Message { foo: 1, bar: 2 });
        }

        // new subscription should only get the latest state (it is up to the Topic trait)
        let mut y = pubsub.subscribe(SubscriptionReq::Bar(2)).unwrap();
        assert_eq!(y.recv().await.map(|x| x.foo), Some(1));
    }
}

//! Publish/Subscribe core
//!
//! This module defines the transport-agnostic pub/sub primitives used by both
//! mint and wallet components. The design prioritizes:
//!
//! - **Request coalescing**: multiple local subscribers to the same remote topic
//!   result in a single upstream subscription, with local fan‑out.
//! - **Latest-on-subscribe** (NUT-17): on (re)subscription, the most recent event
//!   is fetched and delivered before streaming new ones.
//! - **Backpressure-aware delivery**: bounded channels + drop policies prevent
//!   a slow consumer from stalling the whole pipeline.
//! - **Resilience**: automatic reconnect with exponential backoff; WebSocket
//!   streaming when available, HTTP long-poll fallback otherwise.
//!
//! Terms used throughout the module:
//! - **Event**: a domain object that maps to one or more `Topic`s via `Event::get_topics`.
//! - **Topic**: an index/type that defines storage and matching semantics.
//! - **SubscriptionRequest**: a domain-specific filter that can be converted into
//!   low-level transport messages (e.g., WebSocket subscribe frames).
//! - **Spec**: type bundle tying `Event`, `Topic`, `SubscriptionId`, and serialization.

mod error;
mod limits;
mod pubsub;
pub mod remote_consumer;
mod subscriber;
mod types;

pub use self::error::Error;
pub use self::limits::PubsubLimits;
pub use self::pubsub::Pubsub;
pub use self::subscriber::{Subscriber, SubscriptionCounters, SubscriptionRequest};
pub use self::types::*;

#[cfg(test)]
mod test {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, RwLock};

    use serde::{Deserialize, Serialize};
    use tokio::sync::Notify;

    use super::subscriber::SubscriptionRequest;
    use super::{Error, Event, Pubsub, PubsubLimits, Spec, Subscriber};

    #[derive(Clone, Debug, Serialize, Eq, PartialEq, Deserialize)]
    pub struct Message {
        pub foo: u64,
        pub bar: u64,
    }

    #[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Deserialize, Serialize)]
    pub enum IndexTest {
        Foo(u64),
        Bar(u64),
    }

    impl Event for Message {
        type Topic = IndexTest;

        fn get_topics(&self) -> Vec<Self::Topic> {
            vec![IndexTest::Foo(self.foo), IndexTest::Bar(self.bar)]
        }
    }

    pub struct CustomPubSub {
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

    #[derive(Debug, Clone)]
    pub enum SubscriptionReq {
        Foo(u64),
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

    /// Requests a fixed number of distinct topics, to exercise the topic budget.
    #[derive(Debug, Clone)]
    pub struct WideSubscriptionReq(pub u64);

    impl SubscriptionRequest for WideSubscriptionReq {
        type Topic = IndexTest;

        type SubscriptionId = String;

        fn try_get_topics(&self) -> Result<Vec<Self::Topic>, Error> {
            Ok((0..self.0).map(IndexTest::Foo).collect())
        }

        fn subscription_name(&self) -> Arc<Self::SubscriptionId> {
            Arc::new("wide".to_owned())
        }
    }

    /// Requests the same topic twice, to pin down duplicate accounting.
    #[derive(Debug, Clone)]
    pub struct DuplicateSubscriptionReq;

    impl SubscriptionRequest for DuplicateSubscriptionReq {
        type Topic = IndexTest;

        type SubscriptionId = String;

        fn try_get_topics(&self) -> Result<Vec<Self::Topic>, Error> {
            Ok(vec![IndexTest::Foo(1), IndexTest::Foo(1)])
        }

        fn subscription_name(&self) -> Arc<Self::SubscriptionId> {
            Arc::new("duplicate".to_owned())
        }
    }

    /// Records how many backfills run at once and parks until released.
    pub struct BlockingPubSub {
        pub running: AtomicUsize,
        pub peak: AtomicUsize,
        pub completed: AtomicBool,
        pub release: Notify,
    }

    #[async_trait::async_trait]
    impl Spec for BlockingPubSub {
        type Topic = IndexTest;

        type Event = Message;

        type SubscriptionId = String;

        type Context = ();

        fn new_instance(_context: Self::Context) -> Arc<Self> {
            Arc::new(Self {
                running: AtomicUsize::new(0),
                peak: AtomicUsize::new(0),
                completed: AtomicBool::new(false),
                release: Notify::new(),
            })
        }

        async fn fetch_events(
            self: &Arc<Self>,
            _topics: Vec<<Self::Event as Event>::Topic>,
            _reply_to: Subscriber<Self>,
        ) {
            let running = self.running.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(running, Ordering::SeqCst);

            self.release.notified().await;

            self.running.fetch_sub(1, Ordering::SeqCst);
            self.completed.store(true, Ordering::SeqCst);
        }
    }

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
        assert_eq!(pubsub.registered_topics(), 0);
    }

    fn bounded_pubsub(max_topics: usize) -> Pubsub<CustomPubSub> {
        Pubsub::with_limits(
            CustomPubSub::new_instance(()),
            PubsubLimits {
                max_topics,
                ..PubsubLimits::default()
            },
        )
    }

    #[tokio::test]
    async fn topic_budget_rejects_over_limit() {
        let pubsub = bounded_pubsub(2);

        let _first = pubsub.subscribe(SubscriptionReq::Foo(1)).unwrap();
        let _second = pubsub.subscribe(SubscriptionReq::Foo(2)).unwrap();
        assert_eq!(pubsub.registered_topics(), 2);

        assert!(matches!(
            pubsub.subscribe(SubscriptionReq::Foo(3)),
            Err(Error::TooManyTopics)
        ));
        assert_eq!(pubsub.registered_topics(), 2);
        assert_eq!(pubsub.active_subscribers(), 2);
    }

    #[tokio::test]
    async fn a_single_request_cannot_exceed_the_whole_budget() {
        let pubsub = bounded_pubsub(4);

        assert!(matches!(
            pubsub.subscribe(WideSubscriptionReq(5)),
            Err(Error::TooManyTopics)
        ));
        assert_eq!(pubsub.registered_topics(), 0);

        let _fits = pubsub.subscribe(WideSubscriptionReq(4)).unwrap();
        assert_eq!(pubsub.registered_topics(), 4);
    }

    #[tokio::test]
    async fn topic_budget_is_released_on_drop() {
        let pubsub = bounded_pubsub(2);

        let first = pubsub.subscribe(SubscriptionReq::Foo(1)).unwrap();
        let second = pubsub.subscribe(SubscriptionReq::Foo(2)).unwrap();
        assert!(pubsub.subscribe(SubscriptionReq::Foo(3)).is_err());

        drop(first);
        drop(second);

        assert_eq!(pubsub.registered_topics(), 0);
        pubsub
            .subscribe(SubscriptionReq::Foo(3))
            .expect("budget is available again");
    }

    /// Duplicate topics collapse to one index entry but must reserve and release
    /// the same count, or the budget would drift down over time.
    #[tokio::test]
    async fn duplicate_topics_reserve_and_release_the_same_count() {
        let pubsub = bounded_pubsub(4);

        let subscription = pubsub.subscribe(DuplicateSubscriptionReq).unwrap();
        assert_eq!(pubsub.registered_topics(), 2);

        drop(subscription);
        assert_eq!(pubsub.registered_topics(), 0);
    }

    #[tokio::test]
    async fn backfill_concurrency_is_bounded() {
        let inner = BlockingPubSub::new_instance(());
        let pubsub = Pubsub::with_limits(
            inner.clone(),
            PubsubLimits {
                max_concurrent_backfills: 1,
                ..PubsubLimits::default()
            },
        );

        let _subscriptions = (0..4)
            .map(|n| pubsub.subscribe(SubscriptionReq::Foo(n)).unwrap())
            .collect::<Vec<_>>();

        for _ in 0..8 {
            tokio::task::yield_now().await;
        }

        assert_eq!(inner.peak.load(Ordering::SeqCst), 1);
        assert_eq!(pubsub.available_backfill_slots(), 0);
    }

    #[tokio::test]
    async fn backfill_is_aborted_when_the_subscription_drops() {
        let inner = BlockingPubSub::new_instance(());
        let pubsub = Pubsub::new(inner.clone());

        let subscription = pubsub.subscribe(SubscriptionReq::Foo(1)).unwrap();
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        assert_eq!(inner.running.load(Ordering::SeqCst), 1);

        drop(subscription);
        inner.release.notify_waiters();
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }

        assert!(
            !inner.completed.load(Ordering::SeqCst),
            "backfill should be aborted rather than run to completion"
        );
        assert_eq!(
            pubsub.available_backfill_slots(),
            PubsubLimits::default().max_concurrent_backfills
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

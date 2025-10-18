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
mod pubsub;
pub mod remote_consumer;
mod subscriber;
mod types;

pub use self::error::Error;
pub use self::pubsub::Pubsub;
pub use self::subscriber::{Subscriber, SubscriptionRequest};
pub use self::types::*;

#[cfg(test)]
mod test {
    use std::collections::HashMap;
    use std::sync::{Arc, RwLock};

    use serde::{Deserialize, Serialize};

    use super::subscriber::SubscriptionRequest;
    use super::{Error, Event, Pubsub, Spec, Subscriber};

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

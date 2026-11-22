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
    use std::time::Duration;

    use serde::{Deserialize, Serialize};
    use tokio::sync::Notify;
    use tokio::time::timeout;

    use super::subscriber::SubscriptionRequest;
    use super::{Error, Event, Pubsub, Spec, Subscriber};

    #[derive(Clone, Debug, Serialize, Eq, PartialEq, Deserialize)]
    pub struct Message {
        pub foo: u64,
        pub bar: u64,
        #[serde(default)]
        pub done: bool,
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

        fn is_final(&self) -> bool {
            self.done
        }
    }

    pub struct CustomPubSub {
        pub storage: Arc<RwLock<HashMap<IndexTest, Message>>>,
        backfill_gate: Option<(Notify, Notify)>,
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
                backfill_gate: None,
            })
        }

        async fn fetch_events(
            self: &Arc<Self>,
            topics: Vec<<Self::Event as Event>::Topic>,
            reply_to: Subscriber<Self>,
        ) where
            Self: Sized,
        {
            let snapshots = {
                let storage = self.storage.read().unwrap();
                topics
                    .into_iter()
                    .filter_map(|topic| storage.get(&topic).cloned())
                    .collect::<Vec<_>>()
            };
            if let Some((ready, resume)) = &self.backfill_gate {
                ready.notify_one();
                resume.notified().await;
            }

            for value in snapshots {
                reply_to.send_snapshot(value);
            }
        }
    }

    #[derive(Debug, Clone)]
    pub enum SubscriptionReq {
        Foo(u64),
        Bar(u64),
        FooAndBar(u64, u64),
    }

    impl SubscriptionRequest for SubscriptionReq {
        type Topic = IndexTest;

        type SubscriptionId = String;

        fn try_get_topics(&self) -> Result<Vec<Self::Topic>, Error> {
            Ok(match self {
                SubscriptionReq::Bar(n) => vec![IndexTest::Bar(*n)],
                SubscriptionReq::Foo(n) => vec![IndexTest::Foo(*n)],
                SubscriptionReq::FooAndBar(foo, bar) => {
                    vec![IndexTest::Foo(*foo), IndexTest::Bar(*bar)]
                }
            })
        }

        fn subscription_name(&self) -> Arc<Self::SubscriptionId> {
            Arc::new("test".to_owned())
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

        let _ = pubsub.publish_now(Message {
            foo: 2,
            bar: 1,
            done: false,
        });
        let _ = pubsub.publish_now(Message {
            foo: 2,
            bar: 2,
            done: false,
        });

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
    async fn final_live_update_suppresses_only_its_own_snapshot() {
        let stale = Message {
            foo: 2,
            bar: 1,
            done: false,
        };
        let live = Message {
            foo: 2,
            bar: 3,
            done: true,
        };
        let other = Message {
            foo: 3,
            bar: 4,
            done: false,
        };
        let spec = Arc::new(CustomPubSub {
            storage: Arc::new(RwLock::new(HashMap::from([
                (IndexTest::Foo(2), stale),
                (IndexTest::Bar(4), other.clone()),
            ]))),
            backfill_gate: Some((Notify::new(), Notify::new())),
        });
        let (ready, resume) = spec.backfill_gate.as_ref().unwrap();
        let pubsub = Pubsub::new(spec.clone());
        let mut subscriber = pubsub.subscribe(SubscriptionReq::FooAndBar(2, 4)).unwrap();

        timeout(Duration::from_secs(5), async {
            ready.notified().await;
            pubsub.publish_now(live.clone()).unwrap();
            assert_eq!(subscriber.recv().await, Some(live));
            resume.notify_one();
            assert_eq!(subscriber.recv().await, Some(other));
        })
        .await
        .expect("subscription should receive the live update and the unrelated snapshot");
    }

    #[tokio::test]
    async fn non_final_live_update_keeps_snapshot() {
        let snapshot = Message {
            foo: 2,
            bar: 3,
            done: false,
        };
        let delayed_live = Message {
            foo: 2,
            bar: 1,
            done: false,
        };
        let spec = Arc::new(CustomPubSub {
            storage: Arc::new(RwLock::new(HashMap::from([(
                IndexTest::Foo(2),
                snapshot.clone(),
            )]))),
            backfill_gate: Some((Notify::new(), Notify::new())),
        });
        let (ready, resume) = spec.backfill_gate.as_ref().unwrap();
        let pubsub = Pubsub::new(spec.clone());
        let mut subscriber = pubsub.subscribe(SubscriptionReq::Foo(2)).unwrap();

        timeout(Duration::from_secs(5), async {
            ready.notified().await;
            // A delayed live publish may carry a state older than the snapshot.
            pubsub.publish_now(delayed_live.clone()).unwrap();
            assert_eq!(subscriber.recv().await, Some(delayed_live));
            resume.notify_one();
            assert_eq!(subscriber.recv().await, Some(snapshot));
        })
        .await
        .expect("subscription should receive the snapshot after a non-final live update");
    }

    #[tokio::test]
    async fn read_from_storage() {
        let x = CustomPubSub::new_instance(());
        let storage = x.storage.clone();

        let pubsub = Pubsub::new(x);

        {
            // set previous value
            let mut s = storage.write().unwrap();
            s.insert(
                IndexTest::Bar(2),
                Message {
                    foo: 3,
                    bar: 2,
                    done: false,
                },
            );
        }

        let mut subscriber = pubsub.subscribe(SubscriptionReq::Bar(2)).unwrap();

        // Just should receive the latest
        assert_eq!(subscriber.recv().await.map(|x| x.foo), Some(3));

        // realtime delivery test
        let _ = pubsub.publish_now(Message {
            foo: 1,
            bar: 2,
            done: false,
        });
        assert_eq!(subscriber.recv().await.map(|x| x.foo), Some(1));

        {
            // set previous value
            let mut s = storage.write().unwrap();
            s.insert(
                IndexTest::Bar(2),
                Message {
                    foo: 1,
                    bar: 2,
                    done: false,
                },
            );
        }

        // new subscription should only get the latest state (it is up to the Topic trait)
        let mut y = pubsub.subscribe(SubscriptionReq::Bar(2)).unwrap();
        assert_eq!(y.recv().await.map(|x| x.foo), Some(1));
    }
}

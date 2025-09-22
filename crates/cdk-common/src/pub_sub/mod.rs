//! Publish–subscribe manager.
//!
//! This is a event-agnostic Publish-subscriber producer and consumer.
//!
//! This is a generic implementation for
//! [NUT-17](<https://github.com/cashubtc/nuts/blob/main/17.md>) with a type
//! agnostic Publish-subscribe manager.
//!
//! The manager has a method for subscribers to subscribe to events with a
//! generic type that must be converted to a vector of indexes.
//!
//! Events are also generic that should implement the `Indexable` trait.

mod error;
pub mod event;
mod pubsub;
pub mod remote_consumer;
mod subscriber;

pub use self::error::Error;
pub use self::event::Event;
pub use self::pubsub::{Pubsub, Topic};
pub use self::subscriber::SubscriptionRequest;

#[cfg(test)]
mod test {
    use std::collections::HashMap;
    use std::sync::RwLock;

    use serde::{Deserialize, Serialize};
    use tokio::sync::mpsc;

    use super::pubsub::Topic;
    use super::subscriber::SubscriptionRequest;
    use super::{Error, Event, Pubsub};

    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub struct Message {
        foo: u64,
        bar: u64,
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

    #[derive(Default)]
    pub struct CustomTopic {
        storage: RwLock<HashMap<IndexTest, Message>>,
    }

    #[async_trait::async_trait]
    impl Topic for CustomTopic {
        type SubscriptionName = String;

        type Event = Message;

        async fn fetch_events(
            &self,
            indexes: Vec<<Self::Event as Event>::Topic>,
            sub_name: Self::SubscriptionName,
            reply_to: mpsc::Sender<(Self::SubscriptionName, Self::Event)>,
        ) {
            let storage = self.storage.read().unwrap();

            for index in indexes {
                if let Some(value) = storage.get(&index) {
                    let _ = reply_to.try_send((sub_name.clone(), value.clone()));
                }
            }
        }

        /// Store events or replace them
        async fn store_events(&self, event: Self::Event) {
            let mut storage = self.storage.write().unwrap();
            for index in event.get_topics() {
                storage.insert(index, event.clone());
            }
        }
    }

    #[derive(Clone)]
    pub enum SubscriptionReq {
        Foo(u64),
        Bar(u64),
    }

    impl SubscriptionRequest for SubscriptionReq {
        type Topic = IndexTest;

        type SubscriptionName = String;

        fn try_get_topics(&self) -> Result<Vec<Self::Topic>, Error> {
            Ok(vec![match self {
                SubscriptionReq::Bar(n) => IndexTest::Bar(*n),
                SubscriptionReq::Foo(n) => IndexTest::Foo(*n),
            }])
        }

        fn subscription_name(&self) -> Self::SubscriptionName {
            "test".to_owned()
        }
    }

    #[tokio::test]
    async fn delivery_twice_realtime() {
        let pubsub = Pubsub::new(CustomTopic::default());

        assert_eq!(pubsub.active_subscribers(), 0);

        let mut subscriber = pubsub.subscribe(SubscriptionReq::Foo(2)).unwrap();

        assert_eq!(pubsub.active_subscribers(), 1);

        let _ = pubsub.publish_sync(Message { foo: 2, bar: 1 });
        let _ = pubsub.publish_sync(Message { foo: 2, bar: 2 });

        assert_eq!(subscriber.recv().await.map(|x| x.bar), Some(1));
        assert_eq!(subscriber.recv().await.map(|x| x.bar), Some(2));
        assert!(subscriber.try_recv().is_none());

        drop(subscriber);

        assert_eq!(pubsub.active_subscribers(), 0);
    }

    #[tokio::test]
    async fn store_events_once_per_index() {
        let pubsub = Pubsub::new(CustomTopic::default());
        let _ = pubsub.publish_sync(Message { foo: 1, bar: 2 });
        let _ = pubsub.publish_sync(Message { foo: 3, bar: 2 });

        let mut subscriber = pubsub.subscribe(SubscriptionReq::Bar(2)).unwrap();

        // Just should receive the latest
        assert_eq!(subscriber.recv().await.map(|x| x.foo), Some(3));

        // realtime delivery test
        pubsub.publish(Message { foo: 1, bar: 2 });
        assert_eq!(subscriber.recv().await.map(|x| x.foo), Some(1));

        // new subscription should only get the latest state (it is up to the Topic trait)
        let mut y = pubsub.subscribe(SubscriptionReq::Bar(2)).unwrap();
        assert_eq!(y.recv().await.map(|x| x.foo), Some(1));
    }
}

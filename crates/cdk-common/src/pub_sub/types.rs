//! Pubsub Event definition
//!
//! The Pubsub Event defines the Topic struct and how an event can be converted to Topics.

use std::hash::Hash;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::Serialize;

use super::Subscriber;

/// Pubsub settings
#[async_trait::async_trait]
pub trait Spec: Send + Sync {
    /// Topic
    type Topic: Send
        + Sync
        + Clone
        + Eq
        + PartialEq
        + Ord
        + PartialOrd
        + Hash
        + Send
        + Sync
        + DeserializeOwned
        + Serialize;

    /// Event
    type Event: Event<Topic = Self::Topic>
        + Send
        + Sync
        + Eq
        + PartialEq
        + DeserializeOwned
        + Serialize;

    /// Subscription Id
    type SubscriptionId: Clone
        + Default
        + Eq
        + PartialEq
        + Ord
        + PartialOrd
        + Hash
        + Send
        + Sync
        + DeserializeOwned
        + Serialize;

    /// Create a new context
    type Context;

    /// Create a new instance from a given context
    fn new_instance(context: Self::Context) -> Arc<Self>
    where
        Self: Sized;

    /// Callback function that is called on new subscriptions, to back-fill optionally the previous
    /// events
    async fn fetch_events(
        self: &Arc<Self>,
        topics: Vec<<Self::Event as Event>::Topic>,
        reply_to: Subscriber<Self>,
    ) where
        Self: Sized;
}

/// Event trait
pub trait Event: Clone + Send + Sync + Eq + PartialEq + DeserializeOwned + Serialize {
    /// Generic Topic
    ///
    /// It should be serializable/deserializable to be stored in the database layer and it should
    /// also be sorted in a BTree for in-memory matching
    type Topic;

    /// To topics
    fn get_topics(&self) -> Vec<Self::Topic>;
}

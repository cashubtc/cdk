//! Pub-sub producer

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, RwLock};

use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::sync::mpsc;

use super::event::Event;
use super::subscriber::{ActiveSubscription, SubscriptionRequest};
use super::Error;

/// Event producer definition
///
/// This trait defines events to be broadcasted. Events and subscription requests are converted into
/// a vector of topics.
///
/// Matching events with subscriptions, through the topics are broadcasted in real time.
///
/// When a new subscription request is created, a deferred call to `fetch_events` is executed to
/// fetch from a persistent medium the current events to broadcast.
#[async_trait::async_trait]
pub trait Topic: Send + Sync {
    /// Subscription ID
    type SubscriptionName: Debug
        + Clone
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

    /// An Event should be Indexable
    type Event: Event + Debug + Send + Sync + DeserializeOwned + Serialize;

    /// Called when a new subscription is created. The function is responsible to not yield the same
    async fn fetch_events(
        &self,
        topics: Vec<<Self::Event as Event>::Topic>,
        sub_name: Self::SubscriptionName,
        reply_to: mpsc::Sender<(Self::SubscriptionName, Self::Event)>,
    );

    /// Store events or replace them
    async fn store_events(&self, event: Self::Event);
}

/// Default channel size for subscription buffering
pub const DEFAULT_CHANNEL_SIZE: usize = 1_000;

/// Internal Index Tree
pub type TopicTree<P> = Arc<
    RwLock<
        BTreeMap<
            // Index with a subscription unique ID
            (<<P as Topic>::Event as Event>::Topic, usize),
            (
                <P as Topic>::SubscriptionName, // Subscription ID, as given by the client, more like a name
                mpsc::Sender<(<P as Topic>::SubscriptionName, <P as Topic>::Event)>, // Sender
            ),
        >,
    >,
>;

/// Manager
pub struct Pubsub<P>
where
    P: Topic + 'static,
{
    inner: Arc<P>,
    listeners_topics: TopicTree<P>,
    unique_subscription_counter: AtomicUsize,
    active_subscribers: Arc<AtomicUsize>,
}

impl<P> Pubsub<P>
where
    P: Topic + 'static,
{
    /// Create a new instance
    pub fn new(inner: P) -> Self {
        Self {
            inner: Arc::new(inner),
            listeners_topics: Default::default(),
            unique_subscription_counter: 0.into(),
            active_subscribers: Arc::new(0.into()),
        }
    }

    /// Total number of active subscribers, it is not the number of active topics being subscribed
    pub fn active_subscribers(&self) -> usize {
        self.active_subscribers
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Publish an event to all listenrs
    fn publish_internal(
        event: P::Event,
        listeners_index: &TopicTree<P>,
        inner: Arc<P>,
    ) -> Result<(), Error> {
        let index_storage = listeners_index.read().map_err(|_| Error::Poison)?;
        let mut sent = HashSet::new();
        for index in event.get_topics() {
            for ((subscription_index, unique_id), (subscription_id, sender)) in
                index_storage.range((index.clone(), 0)..)
            {
                if subscription_index.cmp(&index) != Ordering::Equal {
                    break;
                }
                if sent.contains(&unique_id) {
                    continue;
                }
                sent.insert(unique_id);
                let _ = sender.try_send((subscription_id.clone(), event.clone()));
            }
        }
        drop(index_storage);

        tokio::spawn(async move {
            inner.store_events(event).await;
        });

        Ok(())
    }

    /// Broadcast an event to all listeners
    #[inline(always)]
    pub fn publish<E>(&self, event: E)
    where
        E: Into<P::Event>,
    {
        let topics = self.listeners_topics.clone();
        let inner = self.inner.clone();
        let event = event.into();

        tokio::spawn(async move { Self::publish_internal(event, &topics, inner) });
    }

    /// Broadcast an event to all listeners right away, blocking the current thread
    ///
    /// This function takes an Arc to the storage struct, the event_id, the kind
    /// and the vent to broadcast
    #[inline(always)]
    pub fn publish_sync<E>(&self, event: E) -> Result<(), Error>
    where
        E: Into<P::Event>,
    {
        let event = event.into();
        Self::publish_internal(event, &self.listeners_topics, self.inner.clone())
    }

    /// Subscribe proving custom sender/receiver mpsc
    #[inline(always)]
    pub fn subscribe_with<I>(
        &self,
        request: I,
        sender: mpsc::Sender<(P::SubscriptionName, P::Event)>,
        receiver: Option<mpsc::Receiver<(P::SubscriptionName, P::Event)>>,
    ) -> Result<ActiveSubscription<P>, Error>
    where
        I: SubscriptionRequest<
            Topic = <P::Event as Event>::Topic,
            SubscriptionName = P::SubscriptionName,
        >,
    {
        let mut index_storage = self.listeners_topics.write().map_err(|_| Error::Poison)?;
        let subscription_internal_id = self
            .unique_subscription_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        self.active_subscribers
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let subscription_name = request.subscription_name();
        let subscribed_to = request.try_get_topics()?;

        for index in subscribed_to.iter() {
            index_storage.insert(
                (index.clone(), subscription_internal_id),
                (subscription_name.clone(), sender.clone()),
            );
        }
        drop(index_storage);

        let inner = self.inner.clone();
        let subscribed_to_for_spawn = subscribed_to.clone();
        let sub_name = subscription_name.clone();
        tokio::spawn(async move {
            inner
                .fetch_events(subscribed_to_for_spawn, sub_name, sender)
                .await;
        });

        Ok(ActiveSubscription::new(
            subscription_internal_id,
            subscription_name,
            self.active_subscribers.clone(),
            self.listeners_topics.clone(),
            subscribed_to,
            receiver,
        ))
    }

    /// Subscribe
    pub fn subscribe<I>(&self, request: I) -> Result<ActiveSubscription<P>, Error>
    where
        I: SubscriptionRequest<
            Topic = <P::Event as Event>::Topic,
            SubscriptionName = P::SubscriptionName,
        >,
    {
        let (sender, receiver) = mpsc::channel(DEFAULT_CHANNEL_SIZE);
        self.subscribe_with(request, sender, Some(receiver))
    }
}

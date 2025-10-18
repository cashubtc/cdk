//! Pub-sub producer

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

use parking_lot::RwLock;
use tokio::sync::mpsc;

use super::subscriber::{ActiveSubscription, SubscriptionRequest};
use super::{Error, Event, Spec, Subscriber};

/// Default channel size for subscription buffering
pub const DEFAULT_CHANNEL_SIZE: usize = 10_000;

/// Subscriber Receiver
pub type SubReceiver<S> = mpsc::Receiver<(Arc<<S as Spec>::SubscriptionId>, <S as Spec>::Event)>;

/// Internal Index Tree
pub type TopicTree<T> = Arc<
    RwLock<
        BTreeMap<
            // Index with a subscription unique ID
            (<T as Spec>::Topic, usize),
            Subscriber<T>,
        >,
    >,
>;

/// Manager
pub struct Pubsub<S>
where
    S: Spec + 'static,
{
    inner: Arc<S>,
    listeners_topics: TopicTree<S>,
    unique_subscription_counter: AtomicUsize,
    active_subscribers: Arc<AtomicUsize>,
}

impl<S> Pubsub<S>
where
    S: Spec + 'static,
{
    /// Create a new instance
    pub fn new(inner: Arc<S>) -> Self {
        Self {
            inner,
            listeners_topics: Default::default(),
            unique_subscription_counter: 0.into(),
            active_subscribers: Arc::new(0.into()),
        }
    }

    /// Total number of active subscribers, it is not the number of active topics being subscribed
    pub fn active_subscribers(&self) -> usize {
        self.active_subscribers
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Publish an event to all listenrs
    #[inline(always)]
    fn publish_internal(event: S::Event, listeners_index: &TopicTree<S>) -> Result<(), Error> {
        let index_storage = listeners_index.read();

        let mut sent = HashSet::new();
        for topic in event.get_topics() {
            for ((subscription_index, unique_id), sender) in
                index_storage.range((topic.clone(), 0)..)
            {
                if subscription_index.cmp(&topic) != Ordering::Equal {
                    break;
                }
                if sent.contains(&unique_id) {
                    continue;
                }
                sent.insert(unique_id);
                sender.send(event.clone());
            }
        }

        Ok(())
    }

    /// Broadcast an event to all listeners
    #[inline(always)]
    pub fn publish<E>(&self, event: E)
    where
        E: Into<S::Event>,
    {
        let topics = self.listeners_topics.clone();
        let event = event.into();

        #[cfg(not(target_arch = "wasm32"))]
        tokio::spawn(async move {
            let _ = Self::publish_internal(event, &topics);
        });

        #[cfg(target_arch = "wasm32")]
        wasm_bindgen_futures::spawn_local(async move {
            let _ = Self::publish_internal(event, &topics);
        });
    }

    /// Broadcast an event to all listeners right away, blocking the current thread
    ///
    /// This function takes an Arc to the storage struct, the event_id, the kind
    /// and the vent to broadcast
    #[inline(always)]
    pub fn publish_now<E>(&self, event: E) -> Result<(), Error>
    where
        E: Into<S::Event>,
    {
        let event = event.into();
        Self::publish_internal(event, &self.listeners_topics)
    }

    /// Subscribe proving custom sender/receiver mpsc
    #[inline(always)]
    pub fn subscribe_with<I>(
        &self,
        request: I,
        sender: &mpsc::Sender<(Arc<I::SubscriptionId>, S::Event)>,
        receiver: Option<SubReceiver<S>>,
    ) -> Result<ActiveSubscription<S>, Error>
    where
        I: SubscriptionRequest<
            Topic = <S::Event as Event>::Topic,
            SubscriptionId = S::SubscriptionId,
        >,
    {
        let subscription_name = request.subscription_name();
        let sender = Subscriber::new(subscription_name.clone(), sender);
        let mut index_storage = self.listeners_topics.write();
        let subscription_internal_id = self
            .unique_subscription_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        self.active_subscribers
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let subscribed_to = request.try_get_topics()?;

        for index in subscribed_to.iter() {
            index_storage.insert((index.clone(), subscription_internal_id), sender.clone());
        }
        drop(index_storage);

        let inner = self.inner.clone();
        let subscribed_to_for_spawn = subscribed_to.clone();

        #[cfg(not(target_arch = "wasm32"))]
        tokio::spawn(async move {
            // TODO: Ignore topics broadcasted from fetch_events _if_ any real time has been broadcasted already.
            inner.fetch_events(subscribed_to_for_spawn, sender).await;
        });

        #[cfg(target_arch = "wasm32")]
        wasm_bindgen_futures::spawn_local(async move {
            inner.fetch_events(subscribed_to_for_spawn, sender).await;
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
    pub fn subscribe<I>(&self, request: I) -> Result<ActiveSubscription<S>, Error>
    where
        I: SubscriptionRequest<
            Topic = <S::Event as Event>::Topic,
            SubscriptionId = S::SubscriptionId,
        >,
    {
        let (sender, receiver) = mpsc::channel(DEFAULT_CHANNEL_SIZE);
        self.subscribe_with(request, &sender, Some(receiver))
    }
}

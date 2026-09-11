//! Pub-sub producer

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::Arc;

use parking_lot::RwLock;
use tokio::sync::{mpsc, Semaphore};

use super::limits::PubsubLimits;
use super::subscriber::{ActiveSubscription, SubscriptionCounters, SubscriptionRequest};
use super::{Error, Event, Spec, Subscriber};
use crate::task::spawn;

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
#[allow(missing_debug_implementations)]
pub struct Pubsub<S>
where
    S: Spec + 'static,
{
    inner: Arc<S>,
    listeners_topics: TopicTree<S>,
    unique_subscription_counter: AtomicUsize,
    /// Topics are counted per requested topic rather than per distinct index
    /// key, so a request with duplicate filters reserves and releases the same
    /// number and the two can never drift.
    counters: Arc<SubscriptionCounters>,
    limits: PubsubLimits,
    backfill_slots: Arc<Semaphore>,
}

impl<S> Pubsub<S>
where
    S: Spec + 'static,
{
    /// Create a new instance with [`PubsubLimits::default`]
    pub fn new(inner: Arc<S>) -> Self {
        Self::with_limits(inner, PubsubLimits::default())
    }

    /// Create a new instance whose shared resources are capped by `limits`
    pub fn with_limits(inner: Arc<S>, limits: PubsubLimits) -> Self {
        Self {
            inner,
            listeners_topics: Default::default(),
            unique_subscription_counter: 0.into(),
            counters: Arc::new(SubscriptionCounters::default()),
            limits,
            backfill_slots: Arc::new(Semaphore::new(limits.max_concurrent_backfills)),
        }
    }

    /// Limits applied to this instance
    pub fn limits(&self) -> PubsubLimits {
        self.limits
    }

    /// Total number of topic registrations currently held across all subscriptions
    pub fn registered_topics(&self) -> usize {
        self.counters
            .registered_topics
            .load(AtomicOrdering::Relaxed)
    }

    /// Number of backfill slots not currently in use
    pub fn available_backfill_slots(&self) -> usize {
        self.backfill_slots.available_permits()
    }

    /// Total number of active subscribers, it is not the number of active topics being subscribed
    pub fn active_subscribers(&self) -> usize {
        self.counters
            .active_subscribers
            .load(AtomicOrdering::Relaxed)
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

        spawn(async move {
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
        let subscribed_to = request.try_get_topics()?;

        // Reserved after the only fallible step and before any shared state is
        // touched, so a rejected request never needs to unwind a partial insert.
        self.reserve_topics(subscribed_to.len())?;

        let sender = Subscriber::new(subscription_name.clone(), sender);
        let mut index_storage = self.listeners_topics.write();
        let subscription_internal_id = self
            .unique_subscription_counter
            .fetch_add(1, AtomicOrdering::Relaxed);

        self.counters
            .active_subscribers
            .fetch_add(1, AtomicOrdering::Relaxed);

        for index in subscribed_to.iter() {
            index_storage.insert((index.clone(), subscription_internal_id), sender.clone());
        }
        drop(index_storage);

        let inner = self.inner.clone();
        let subscribed_to_for_spawn = subscribed_to.clone();
        let backfill_slots = self.backfill_slots.clone();

        let backfill = spawn(async move {
            // Backfill reads the database and the payment backend once per topic,
            // so a burst of subscriptions has to queue rather than fan out.
            let _permit = match backfill_slots.acquire_owned().await {
                Ok(permit) => permit,
                Err(err) => {
                    tracing::debug!("Backfill slot unavailable, skipping backfill: {err}");
                    return;
                }
            };
            inner.fetch_events(subscribed_to_for_spawn, sender).await;
        });

        Ok(ActiveSubscription::new(
            subscription_internal_id,
            subscription_name,
            self.counters.clone(),
            self.listeners_topics.clone(),
            subscribed_to,
            receiver,
            backfill,
        ))
    }

    /// Atomically claim `requested` slots of the topic budget
    fn reserve_topics(&self, requested: usize) -> Result<(), Error> {
        let mut current = self
            .counters
            .registered_topics
            .load(AtomicOrdering::Relaxed);
        loop {
            let next = current
                .checked_add(requested)
                .filter(|next| *next <= self.limits.max_topics)
                .ok_or(Error::TooManyTopics)?;

            match self.counters.registered_topics.compare_exchange_weak(
                current,
                next,
                AtomicOrdering::AcqRel,
                AtomicOrdering::Acquire,
            ) {
                Ok(_) => return Ok(()),
                Err(actual) => current = actual,
            }
        }
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

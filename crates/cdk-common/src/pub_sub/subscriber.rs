//! Active subscription
use std::collections::HashSet;
use std::fmt;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::mpsc;

use super::pubsub::{SubReceiver, TopicTree};
use super::{Error, Event, Spec};

/// Subscription request
pub trait SubscriptionRequest {
    /// Topics
    type Topic;

    /// Subscription Id
    type SubscriptionId;

    /// Try to get topics from the request
    fn try_get_topics(&self) -> Result<Vec<Self::Topic>, Error>;

    /// Get the subscription name
    fn subscription_name(&self) -> Arc<Self::SubscriptionId>;
}

/// Active Subscription
#[allow(missing_debug_implementations)]
pub struct ActiveSubscription<S>
where
    S: Spec + 'static,
{
    id: usize,
    name: Arc<S::SubscriptionId>,
    active_subscribers: Arc<AtomicUsize>,
    topics: TopicTree<S>,
    subscribed_to: Vec<S::Topic>,
    receiver: Option<SubReceiver<S>>,
}

impl<S> ActiveSubscription<S>
where
    S: Spec + 'static,
{
    /// Creates a new instance
    pub fn new(
        id: usize,
        name: Arc<S::SubscriptionId>,
        active_subscribers: Arc<AtomicUsize>,
        topics: TopicTree<S>,
        subscribed_to: Vec<S::Topic>,
        receiver: Option<SubReceiver<S>>,
    ) -> Self {
        Self {
            id,
            name,
            active_subscribers,
            subscribed_to,
            topics,
            receiver,
        }
    }

    /// Receives the next event
    pub async fn recv(&mut self) -> Option<S::Event> {
        self.receiver.as_mut()?.recv().await.map(|(_, event)| event)
    }

    /// Try receive an event or return None right away
    pub fn try_recv(&mut self) -> Option<S::Event> {
        self.receiver
            .as_mut()?
            .try_recv()
            .ok()
            .map(|(_, event)| event)
    }

    /// Get the subscription name
    pub fn name(&self) -> &S::SubscriptionId {
        &self.name
    }
}

impl<S> Drop for ActiveSubscription<S>
where
    S: Spec + 'static,
{
    fn drop(&mut self) {
        // remove the listener
        let mut topics = self.topics.write();
        for index in self.subscribed_to.drain(..) {
            topics.remove(&(index, self.id));
        }

        // decrement the number of active subscribers
        self.active_subscribers
            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Lightweight sink used by producers to send events to subscribers.
///
/// You usually do not construct a `Subscriber` directly — it is provided to you in
/// the [`Spec::fetch_events`] callback so you can backfill a new subscription.
pub struct Subscriber<S>
where
    S: Spec + 'static,
{
    subscription: Arc<S::SubscriptionId>,
    inner: mpsc::Sender<(Arc<S::SubscriptionId>, S::Event)>,
    delivery: Arc<Mutex<DeliveryState<S>>>,
}

struct DeliveryState<S: Spec> {
    latest: Option<S::Event>,
    snapshot_topics: Option<HashSet<S::Topic>>,
}

impl<S> fmt::Debug for Subscriber<S>
where
    S: Spec + 'static,
    S::SubscriptionId: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Subscriber")
            .field("subscription", &self.subscription)
            .finish_non_exhaustive()
    }
}

impl<S> Clone for Subscriber<S>
where
    S: Spec + 'static,
{
    fn clone(&self) -> Self {
        Self {
            subscription: self.subscription.clone(),
            inner: self.inner.clone(),
            delivery: self.delivery.clone(),
        }
    }
}

impl<S> Subscriber<S>
where
    S: Spec + 'static,
{
    /// Create a new instance
    pub fn new(
        subscription: Arc<S::SubscriptionId>,
        inner: &mpsc::Sender<(Arc<S::SubscriptionId>, S::Event)>,
    ) -> Self {
        Self {
            inner: inner.clone(),
            subscription,
            delivery: Arc::new(Mutex::new(DeliveryState {
                latest: None,
                snapshot_topics: None,
            })),
        }
    }

    /// Send a message
    pub fn send(&self, event: S::Event) {
        self.enqueue(&mut self.delivery.lock(), event);
    }

    /// Queue a backfill snapshot unless it has been superseded.
    ///
    /// While the subscription is backfilling, the snapshot is dropped when a final live event
    /// (see [`Event::is_final`]) has already been queued for every subscribed topic it covers,
    /// so a stale read cannot follow a state that nothing can supersede.
    pub fn send_snapshot(&self, event: S::Event) {
        let mut delivery = self.delivery.lock();
        if let Some(pending) = &delivery.snapshot_topics {
            if !event
                .get_topics()
                .iter()
                .any(|topic| pending.contains(topic))
            {
                return;
            }
        }
        self.enqueue(&mut delivery, event);
    }

    pub(super) fn start_backfill(&self, topics: &[S::Topic]) {
        self.delivery.lock().snapshot_topics = Some(topics.iter().cloned().collect());
    }

    pub(super) fn finish_backfill(&self) {
        self.delivery.lock().snapshot_topics = None;
    }

    pub(super) fn send_live(&self, topics: &[S::Topic], event: S::Event) {
        let mut delivery = self.delivery.lock();
        let is_final = event.is_final();
        if self.enqueue(&mut delivery, event) && is_final {
            if let Some(snapshot_topics) = &mut delivery.snapshot_topics {
                for topic in topics {
                    snapshot_topics.remove(topic);
                }
            }
        }
    }

    fn enqueue(&self, delivery: &mut DeliveryState<S>, event: S::Event) -> bool {
        if delivery.latest.as_ref() == Some(&event) {
            return false;
        }
        let Ok(permit) = self.inner.try_reserve() else {
            return false;
        };
        delivery.latest = Some(event.clone());
        permit.send((self.subscription.clone(), event));
        true
    }
}

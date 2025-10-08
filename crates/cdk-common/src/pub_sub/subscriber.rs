//! Active subscription
use std::fmt::Debug;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use super::pubsub::{SubReceiver, TopicTree};
use super::{Error, Spec};

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

    /// Try receive an event or return Noen right away
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
#[derive(Debug)]
pub struct Subscriber<S>
where
    S: Spec + 'static,
{
    subscription: Arc<S::SubscriptionId>,
    inner: mpsc::Sender<(Arc<S::SubscriptionId>, S::Event)>,
    latest: Arc<Mutex<Option<S::Event>>>,
}

impl<S> Clone for Subscriber<S>
where
    S: Spec + 'static,
{
    fn clone(&self) -> Self {
        Self {
            subscription: self.subscription.clone(),
            inner: self.inner.clone(),
            latest: self.latest.clone(),
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
            latest: Arc::new(Mutex::new(None)),
        }
    }

    /// Send a message
    pub fn send(&self, event: S::Event) {
        let mut latest = if let Ok(reader) = self.latest.lock() {
            reader
        } else {
            let _ = self.inner.try_send((self.subscription.to_owned(), event));
            return;
        };

        if let Some(last_event) = latest.replace(event.clone()) {
            if last_event == event {
                return;
            }
        }

        let _ = self.inner.try_send((self.subscription.to_owned(), event));
    }
}

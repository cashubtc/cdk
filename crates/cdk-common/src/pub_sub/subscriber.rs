//! Active subscription
use std::collections::HashMap;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use super::pubsub::{Topic, TopicTree};
use super::Error;
use crate::pub_sub::event::Event;

/// Subscription request
pub trait SubscriptionRequest: Clone {
    /// Topics
    type Topic;

    /// Subscription name
    type SubscriptionName;

    /// Try to get topics from the request
    fn try_get_topics(&self) -> Result<Vec<Self::Topic>, Error>;

    /// Get the subscription name
    fn subscription_name(&self) -> Self::SubscriptionName;
}

/// Active Subscription
pub struct ActiveSubscription<P>
where
    P: Topic + 'static,
{
    id: usize,
    name: P::SubscriptionName,
    active_subscribers: Arc<AtomicUsize>,
    topics: TopicTree<P>,
    subscribed_to: Vec<<P::Event as Event>::Topic>,
    receiver: Option<mpsc::Receiver<(P::SubscriptionName, P::Event)>>,
}

impl<P> ActiveSubscription<P>
where
    P: Topic + 'static,
{
    /// Creates a new instance
    pub fn new(
        id: usize,
        name: P::SubscriptionName,
        active_subscribers: Arc<AtomicUsize>,
        topics: TopicTree<P>,
        subscribed_to: Vec<<P::Event as Event>::Topic>,
        receiver: Option<mpsc::Receiver<(P::SubscriptionName, P::Event)>>,
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
    pub async fn recv(&mut self) -> Option<P::Event> {
        self.receiver.as_mut()?.recv().await.map(|(_, event)| event)
    }

    /// Try receive an event or return Noen right away
    pub fn try_recv(&mut self) -> Option<P::Event> {
        self.receiver
            .as_mut()?
            .try_recv()
            .ok()
            .map(|(_, event)| event)
    }

    /// Get the subscription name
    pub fn name(&self) -> &P::SubscriptionName {
        &self.name
    }
}

impl<P> Drop for ActiveSubscription<P>
where
    P: Topic + 'static,
{
    fn drop(&mut self) {
        // remove the listener
        let mut topics = self.topics.write().unwrap();
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
/// the [`Topic::fetch_events`] callback so you can backfill a new subscription.
#[derive(Debug)]
pub struct Subscriber<T>
where
    T: Topic + 'static,
{
    inner: mpsc::Sender<(T::SubscriptionName, T::Event)>,
    latest: Arc<Mutex<HashMap<T::SubscriptionName, T::Event>>>,
}

impl<T> Clone for Subscriber<T>
where
    T: Topic + 'static,
{
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            latest: self.latest.clone(),
        }
    }
}

impl<T> Subscriber<T>
where
    T: Topic + 'static,
{
    /// Create a new instance
    pub fn new(inner: &mpsc::Sender<(T::SubscriptionName, T::Event)>) -> Self {
        Self {
            inner: inner.clone(),
            latest: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Send a message
    pub fn send(&self, name: &T::SubscriptionName, event: T::Event) {
        let mut latest = if let Ok(reader) = self.latest.lock() {
            reader
        } else {
            let _ = self.inner.try_send((name.to_owned(), event));
            return;
        };

        if let Some(last_event) = latest.insert(name.clone(), event.clone()) {
            if last_event == event {
                return;
            }
        }

        let _ = self.inner.try_send((name.to_owned(), event));
    }
}

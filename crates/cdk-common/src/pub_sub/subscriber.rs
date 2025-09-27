//! Active subscription
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

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

//! Active subscription
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

use tokio::sync::mpsc;

use super::pubsub::{IndexTree, Topic};
use super::Error;
use crate::pub_sub::index::Indexable;

/// Subscription request
pub trait SubscriptionRequest: Clone {
    /// Indexes
    type Index;

    /// Subscription name
    type SubscriptionName;

    /// Try to get indexes from the request
    fn try_get_indexes(&self) -> Result<Vec<Self::Index>, Error>;

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
    indexes: IndexTree<P>,
    subscribed_to: Vec<<P::Event as Indexable>::Index>,
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
        indexes: IndexTree<P>,
        subscribed_to: Vec<<P::Event as Indexable>::Index>,
        receiver: Option<mpsc::Receiver<(P::SubscriptionName, P::Event)>>,
    ) -> Self {
        Self {
            id,
            name,
            active_subscribers,
            subscribed_to,
            indexes,
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
        let mut indexes = self.indexes.write().unwrap();
        for index in self.subscribed_to.drain(..) {
            indexes.remove(&(index, self.id));
        }

        // decrement the number of active subscribers
        self.active_subscribers
            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

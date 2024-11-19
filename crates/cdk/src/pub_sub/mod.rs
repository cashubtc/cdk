//! Publish–subscribe pattern.
//!
//! This is a generic implementation for
//! [NUT-17(https://github.com/cashubtc/nuts/blob/main/17.md) with a type
//! agnostic Publish-subscribe manager.
//!
//! The manager has a method for subscribers to subscribe to events with a
//! generic type that must be converted to a vector of indexes.
//!
//! Events are also generic that should implement the `Indexable` trait.
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};
use std::fmt::Debug;
use std::ops::{Deref, DerefMut};
use std::str::FromStr;
use std::sync::atomic::{self, AtomicUsize};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, RwLock};
use tokio::task::JoinHandle;

mod index;

pub use index::{Index, Indexable, SubscriptionGlobalId};

type IndexTree<T, I> = Arc<RwLock<BTreeMap<Index<I>, mpsc::Sender<(SubId, T)>>>>;

/// Default size of the remove channel
pub const DEFAULT_REMOVE_SIZE: usize = 10_000;

/// Default channel size for subscription buffering
pub const DEFAULT_CHANNEL_SIZE: usize = 10;

#[async_trait::async_trait]
/// On New Subscription trait
///
/// This trait is optional and it is used to notify the application when a new
/// subscription is created. This is useful when the application needs to send
/// the initial state to the subscriber upon subscription
pub trait OnNewSubscription {
    /// Index type
    type Index;
    /// Subscription event type
    type Event;

    /// Called when a new subscription is created
    async fn on_new_subscription(
        &self,
        request: &[&Self::Index],
    ) -> Result<Vec<Self::Event>, String>;
}

/// Subscription manager
///
/// This object keep track of all subscription listener and it is also
/// responsible for broadcasting events to all listeners
///
/// The content of the notification is not relevant to this scope and it is up
/// to the application, therefore the generic T is used instead of a specific
/// type
pub struct Manager<T, I, F>
where
    T: Indexable<Type = I> + Clone + Send + Sync + 'static,
    I: PartialOrd + Clone + Debug + Ord + Send + Sync + 'static,
    F: OnNewSubscription<Index = I, Event = T> + 'static,
{
    indexes: IndexTree<T, I>,
    on_new_subscription: Option<F>,
    unsubscription_sender: mpsc::Sender<(SubId, Vec<Index<I>>)>,
    active_subscriptions: Arc<AtomicUsize>,
    background_subscription_remover: Option<JoinHandle<()>>,
}

impl<T, I, F> Default for Manager<T, I, F>
where
    T: Indexable<Type = I> + Clone + Send + Sync + 'static,
    I: PartialOrd + Clone + Debug + Ord + Send + Sync + 'static,
    F: OnNewSubscription<Index = I, Event = T> + 'static,
{
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel(DEFAULT_REMOVE_SIZE);
        let active_subscriptions: Arc<AtomicUsize> = Default::default();
        let storage: IndexTree<T, I> = Arc::new(Default::default());

        Self {
            background_subscription_remover: Some(tokio::spawn(Self::remove_subscription(
                receiver,
                storage.clone(),
                active_subscriptions.clone(),
            ))),
            on_new_subscription: None,
            unsubscription_sender: sender,
            active_subscriptions,
            indexes: storage,
        }
    }
}

impl<T, I, F> From<F> for Manager<T, I, F>
where
    T: Indexable<Type = I> + Clone + Send + Sync + 'static,
    I: PartialOrd + Clone + Debug + Ord + Send + Sync + 'static,
    F: OnNewSubscription<Index = I, Event = T> + 'static,
{
    fn from(value: F) -> Self {
        let mut manager: Self = Default::default();
        manager.on_new_subscription = Some(value);
        manager
    }
}

impl<T, I, F> Manager<T, I, F>
where
    T: Indexable<Type = I> + Clone + Send + Sync + 'static,
    I: PartialOrd + Clone + Debug + Ord + Send + Sync + 'static,
    F: OnNewSubscription<Index = I, Event = T> + 'static,
{
    #[inline]
    /// Broadcast an event to all listeners
    ///
    /// This function takes an Arc to the storage struct, the event_id, the kind
    /// and the vent to broadcast
    async fn broadcast_impl(storage: &IndexTree<T, I>, event: T) {
        let index_storage = storage.read().await;
        let mut sent = HashSet::new();
        for index in event.to_indexes() {
            for (key, sender) in index_storage.range(index.clone()..) {
                if index.cmp_prefix(key) != Ordering::Equal {
                    break;
                }
                let sub_id = key.unique_id();
                if sent.contains(&sub_id) {
                    continue;
                }
                sent.insert(sub_id);
                let _ = sender.try_send((key.into(), event.clone()));
            }
        }
    }

    /// Broadcasts an event to all listeners
    ///
    /// This public method will not block the caller, it will spawn a new task
    /// instead
    pub fn broadcast(&self, event: T) {
        let storage = self.indexes.clone();
        tokio::spawn(async move {
            Self::broadcast_impl(&storage, event).await;
        });
    }

    /// Broadcasts an event to all listeners
    ///
    /// This method is async and will await for the broadcast to be completed
    pub async fn broadcast_async(&self, event: T) {
        Self::broadcast_impl(&self.indexes, event).await;
    }

    /// Specific of the subscription, this is the abstraction between `subscribe` and `try_subscribe`
    #[inline(always)]
    async fn subscribe_inner(
        &self,
        sub_id: SubId,
        indexes: Vec<Index<I>>,
    ) -> ActiveSubscription<T, I> {
        let (sender, receiver) = mpsc::channel(10);
        if let Some(on_new_subscription) = self.on_new_subscription.as_ref() {
            match on_new_subscription
                .on_new_subscription(&indexes.iter().map(|x| x.deref()).collect::<Vec<_>>())
                .await
            {
                Ok(events) => {
                    for event in events {
                        let _ = sender.try_send((sub_id.clone(), event));
                    }
                }
                Err(err) => {
                    tracing::info!(
                        "Failed to get initial state for subscription: {:?}, {}",
                        sub_id,
                        err
                    );
                }
            }
        }

        let mut index_storage = self.indexes.write().await;
        for index in indexes.clone() {
            index_storage.insert(index, sender.clone());
        }
        drop(index_storage);

        self.active_subscriptions
            .fetch_add(1, atomic::Ordering::Relaxed);

        ActiveSubscription {
            sub_id,
            receiver,
            indexes,
            drop: self.unsubscription_sender.clone(),
        }
    }

    /// Try to subscribe to a specific event
    pub async fn try_subscribe<P: AsRef<SubId> + TryInto<Vec<Index<I>>>>(
        &self,
        params: P,
    ) -> Result<ActiveSubscription<T, I>, P::Error> {
        Ok(self
            .subscribe_inner(params.as_ref().clone(), params.try_into()?)
            .await)
    }

    /// Subscribe to a specific event
    pub async fn subscribe<P: AsRef<SubId> + Into<Vec<Index<I>>>>(
        &self,
        params: P,
    ) -> ActiveSubscription<T, I> {
        self.subscribe_inner(params.as_ref().clone(), params.into())
            .await
    }

    /// Return number of active subscriptions
    pub fn active_subscriptions(&self) -> usize {
        self.active_subscriptions.load(atomic::Ordering::SeqCst)
    }

    /// Task to remove dropped subscriptions from the storage struct
    ///
    /// This task will run in the background (and will be dropped when the [`Manager`]
    /// is) and will remove subscriptions from the storage struct it is dropped.
    async fn remove_subscription(
        mut receiver: mpsc::Receiver<(SubId, Vec<Index<I>>)>,
        storage: IndexTree<T, I>,
        active_subscriptions: Arc<AtomicUsize>,
    ) {
        while let Some((sub_id, indexes)) = receiver.recv().await {
            tracing::info!("Removing subscription: {}", *sub_id);

            active_subscriptions.fetch_sub(1, atomic::Ordering::AcqRel);

            let mut index_storage = storage.write().await;
            for key in indexes {
                index_storage.remove(&key);
            }
            drop(index_storage);
        }
    }
}

/// Manager goes out of scope, stop all background tasks
impl<T, I, F> Drop for Manager<T, I, F>
where
    T: Indexable<Type = I> + Clone + Send + Sync + 'static,
    I: Clone + Debug + PartialOrd + Ord + Send + Sync + 'static,
    F: OnNewSubscription<Index = I, Event = T> + 'static,
{
    fn drop(&mut self) {
        if let Some(handler) = self.background_subscription_remover.take() {
            handler.abort();
        }
    }
}

/// Active Subscription
///
/// This struct is a wrapper around the mpsc::Receiver<Event> and it also used
/// to keep track of the subscription itself. When this struct goes out of
/// scope, it will notify the Manager about it, so it can be removed from the
/// list of active listeners
pub struct ActiveSubscription<T, I>
where
    T: Send + Sync,
    I: Clone + Debug + PartialOrd + Ord + Send + Sync + 'static,
{
    /// The subscription ID
    pub sub_id: SubId,
    indexes: Vec<Index<I>>,
    receiver: mpsc::Receiver<(SubId, T)>,
    drop: mpsc::Sender<(SubId, Vec<Index<I>>)>,
}

impl<T, I> Deref for ActiveSubscription<T, I>
where
    T: Send + Sync,
    I: Clone + Debug + PartialOrd + Ord + Send + Sync + 'static,
{
    type Target = mpsc::Receiver<(SubId, T)>;

    fn deref(&self) -> &Self::Target {
        &self.receiver
    }
}

impl<T, I> DerefMut for ActiveSubscription<T, I>
where
    T: Indexable + Clone + Send + Sync + 'static,
    I: Clone + Debug + PartialOrd + Ord + Send + Sync + 'static,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.receiver
    }
}

/// The ActiveSubscription is Drop out of scope, notify the Manager about it, so
/// it can be removed from the list of active listeners
///
/// Having this in place, we can avoid memory leaks and also makes it super
/// simple to implement the Unsubscribe method
impl<T, I> Drop for ActiveSubscription<T, I>
where
    T: Send + Sync,
    I: Clone + Debug + PartialOrd + Ord + Send + Sync + 'static,
{
    fn drop(&mut self) {
        let _ = self
            .drop
            .try_send((self.sub_id.clone(), self.indexes.drain(..).collect()));
    }
}

/// Subscription Id wrapper
///
/// This is the place to add some sane default (like a max length) to the
/// subscription ID
#[derive(Debug, Clone, Default, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct SubId(String);

impl From<&str> for SubId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for SubId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl FromStr for SubId {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.to_string()))
    }
}

impl Deref for SubId {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
mod test {
    use tokio::sync::mpsc;

    use super::*;

    #[test]
    fn test_active_subscription_drop() {
        let (tx, rx) = mpsc::channel::<(SubId, ())>(10);
        let sub_id = SubId::from("test_sub_id");
        let indexes: Vec<Index<String>> = vec![Index::from(("test".to_string(), sub_id.clone()))];
        let (drop_tx, mut drop_rx) = mpsc::channel(10);

        {
            let _active_subscription = ActiveSubscription {
                sub_id: sub_id.clone(),
                indexes,
                receiver: rx,
                drop: drop_tx,
            };
            // When it goes out of scope, it should notify
        }
        assert_eq!(drop_rx.try_recv().unwrap().0, sub_id); // it should have notified
        assert!(tx.try_send(("foo".into(), ())).is_err()); // subscriber is dropped
    }
}

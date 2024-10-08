//! Subscription manager
//!
//! This is an attempt to implement [NUT-17](https://github.com/cashubtc/nuts/blob/main/17.md)
use serde::{Deserialize, Serialize};
use std::{
    ops::{Deref, DerefMut},
    sync::{atomic::AtomicUsize, Arc},
};
use tokio::{sync::mpsc, task::JoinHandle};

mod storage;

use storage::SubscriptionStorage;

/// Default size of the remove channel
pub const DEFAULT_REMOVE_SIZE: usize = 10;
/// Default channel size for subscription buffering
pub const DEFAULT_CHANNEL_SIZE: usize = 10;

/// Subscription manager
///
/// This object keep track of all subscription listener and it is also
/// responsible for broadcasting events to all listeners
///
/// The content of the notification is not relevant to this scope and it is up
/// to the application, therefore the generic T is used instead of a specific
/// type
pub struct Manager<T>
where
    T: Send + Sync + 'static,
{
    storage: Arc<SubscriptionStorage<T>>,
    unsubscription_sender: mpsc::Sender<SubId>,
    background_tasks: Vec<JoinHandle<()>>,
    counter: AtomicUsize,
}

impl<T> Default for Manager<T>
where
    T: Send + Sync + 'static,
{
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel(DEFAULT_REMOVE_SIZE);
        let storage: Arc<SubscriptionStorage<T>> = Arc::new(Default::default());

        Self {
            background_tasks: vec![tokio::spawn(Self::remove_subscription(
                receiver,
                storage.clone(),
            ))],
            unsubscription_sender: sender,
            storage,
            counter: Default::default(),
        }
    }
}

impl<T> Manager<T>
where
    T: Send + Sync,
{
    /// Subscribe to a specific event
    pub async fn subscribe(&self, mut params: Params) -> ActiveSubscription<T> {
        let mut subscriptions = self.storage.subscriptions.write().await;
        let (sender, receiver) = mpsc::channel(10);

        subscriptions.insert(params.id.clone(), params.clone());
        drop(subscriptions);

        let mut indexes = self.storage.indexes.write().await;
        let sub_internal_id = self
            .counter
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        for filter in params.filters.drain(..) {
            let index = (
                filter,
                params.kind.clone(),
                params.id.clone(),
                sub_internal_id,
            );
            indexes.insert(index, sender.clone());
        }
        drop(indexes);

        ActiveSubscription {
            id: params.id,
            receiver,
            drop: self.unsubscription_sender.clone(),
        }
    }

    /// Task to remove dropped subscriptions from the storage struct
    ///
    /// This task will run in the background (and will dropped when the Manager
    /// is ) and will remove subscriptions from the storage struct it is dropped.
    async fn remove_subscription(
        mut receiver: mpsc::Receiver<SubId>,
        storage: Arc<SubscriptionStorage<T>>,
    ) {
        while let Some(sub_id) = receiver.recv().await {
            tracing::info!("Removing subscription: {}", *sub_id);
            let params = if let Some(params) = storage.subscriptions.write().await.remove(&sub_id) {
                params
            } else {
                tracing::warn!("Subscription not found: {}", *sub_id);
                continue;
            };

            let indexes = storage.indexes.read().await;
            let mut to_remove = vec![];
            for filter in params.filters {
                let index = (filter, params.kind.clone(), params.id.clone(), 0);
                let mut iterator = indexes.range(index..);
                while let Some((key, _)) = iterator.next() {
                    if params.id != key.2 {
                        break;
                    }
                    to_remove.push(key.clone());
                }
            }
            drop(indexes);

            if !to_remove.is_empty() {
                let mut indexes = storage.indexes.write().await;
                for key in to_remove {
                    indexes.remove(&key);
                }
                drop(indexes);
            }
        }
    }
}

/// Manager goes out of scope, stop all background tasks
impl<T> Drop for Manager<T>
where
    T: Send + Sync,
{
    fn drop(&mut self) {
        for task in self.background_tasks.drain(..) {
            task.abort();
        }
    }
}

/// Active Subscription
///
/// This struct is a wrapper around the mpsc::Receiver<Event> and it also used
/// to keep track of the subscription itself. When this struct goes out of
/// scope, it will notify the Manager about it, so it can be removed from the
/// list of active listeners
pub struct ActiveSubscription<T>
where
    T: Send + Sync,
{
    /// The subscription ID
    pub id: SubId,
    receiver: mpsc::Receiver<T>,
    drop: mpsc::Sender<SubId>,
}

impl<T> Deref for ActiveSubscription<T>
where
    T: Send + Sync,
{
    type Target = mpsc::Receiver<T>;

    fn deref(&self) -> &Self::Target {
        &self.receiver
    }
}

impl<T> DerefMut for ActiveSubscription<T>
where
    T: Send + Sync,
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
impl<T> Drop for ActiveSubscription<T>
where
    T: Send + Sync,
{
    fn drop(&mut self) {
        self.drop.try_send(self.id.clone()).unwrap();
    }
}

/// Subscription Parameter according to the standard
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Params {
    kind: Kind,
    filters: Vec<String>,
    #[serde(rename = "subId")]
    id: SubId,
}

/// Subscription Id wrapper
///
/// This is the place to add some sane default (like a max length) to the
/// subscription ID
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct SubId(String);

impl Deref for SubId {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Clone, Eq, Ord, PartialOrd, PartialEq, Hash, Serialize, Deserialize)]
pub enum Kind {
    Bolt11MeltQuote,
    Bolt11MintQuote,
    ProofState,
}

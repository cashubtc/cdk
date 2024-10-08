use std::collections::{BTreeMap, HashMap};
use tokio::sync::{mpsc, RwLock};

/// Index of the subscription
///
/// This index is used to match events with the correct subscription, the last
/// usize is not really used, it is just a place holder to make sure the index
/// is unique
type Index = (String, super::Kind, super::SubId, usize);

pub struct SubscriptionStorage<T>
where
    T: Send + Sync,
{
    pub subscriptions: RwLock<HashMap<super::SubId, super::Params>>,
    pub indexes: RwLock<BTreeMap<Index, mpsc::Sender<T>>>,
}

impl<T> Default for SubscriptionStorage<T>
where
    T: Send + Sync,
{
    fn default() -> Self {
        Self {
            subscriptions: Default::default(),
            indexes: Default::default(),
        }
    }
}

use std::fmt::Debug;
use std::ops::Deref;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::SubId;

/// Indexable trait
pub trait Indexable {
    /// The type of the index, it is unknown and it is up to the Manager's
    /// generic type
    type Type: PartialOrd + Ord + Send + Sync + Debug;

    /// To indexes
    fn to_indexes(&self) -> Vec<Index<Self::Type>>;
}

#[derive(Debug, Ord, PartialOrd, PartialEq, Eq, Clone)]
/// Index
///
/// The Index is a sorted structure that is used to quickly find matches
///
/// The counter is used to make sure each Index is unique, even if the prefix
/// are the same, and also to make sure that earlier indexes matches first
pub struct Index<T>
where
    T: PartialOrd + Ord + Send + Sync + Debug,
{
    prefix: T,
    counter: SubscriptionGlobalId,
    id: super::SubId,
}

impl<T> From<&Index<T>> for super::SubId
where
    T: PartialOrd + Ord + Send + Sync + Debug,
{
    fn from(val: &Index<T>) -> Self {
        val.id.clone()
    }
}

impl<T> Deref for Index<T>
where
    T: PartialOrd + Ord + Send + Sync + Debug,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.prefix
    }
}

impl<T> Index<T>
where
    T: PartialOrd + Ord + Send + Sync + Debug,
{
    /// Compare the
    pub fn cmp_prefix(&self, other: &Index<T>) -> std::cmp::Ordering {
        self.prefix.cmp(&other.prefix)
    }

    /// Returns a globally unique id for the Index
    pub fn unique_id(&self) -> usize {
        self.counter.0
    }
}

impl<T> From<(T, SubId, SubscriptionGlobalId)> for Index<T>
where
    T: PartialOrd + Ord + Send + Sync + Debug,
{
    fn from((prefix, id, counter): (T, SubId, SubscriptionGlobalId)) -> Self {
        Self {
            prefix,
            id,
            counter,
        }
    }
}

impl<T> From<(T, SubId)> for Index<T>
where
    T: PartialOrd + Ord + Send + Sync + Debug,
{
    fn from((prefix, id): (T, SubId)) -> Self {
        Self {
            prefix,
            id,
            counter: Default::default(),
        }
    }
}

impl<T> From<T> for Index<T>
where
    T: PartialOrd + Ord + Send + Sync + Debug,
{
    fn from(prefix: T) -> Self {
        Self {
            prefix,
            id: Default::default(),
            counter: SubscriptionGlobalId(0),
        }
    }
}

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Dummy type
///
/// This is only use so each Index is unique, with the same prefix.
///
/// The prefix is used to leverage the BTree to find things quickly, but each
/// entry/key must be unique, so we use this dummy type to make sure each Index
/// is unique.
///
/// Unique is also used to make sure that the indexes are sorted by creation order
#[derive(Debug, Ord, PartialOrd, PartialEq, Eq, Clone, Copy)]
pub struct SubscriptionGlobalId(usize);

impl Default for SubscriptionGlobalId {
    fn default() -> Self {
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_from_tuple() {
        let sub_id = SubId::from("test_sub_id");
        let prefix = "test_prefix";
        let index: Index<&str> = Index::from((prefix, sub_id.clone()));
        assert_eq!(index.prefix, "test_prefix");
        assert_eq!(index.id, sub_id);
    }

    #[test]
    fn test_index_cmp_prefix() {
        let sub_id = SubId::from("test_sub_id");
        let index1: Index<&str> = Index::from(("a", sub_id.clone()));
        let index2: Index<&str> = Index::from(("b", sub_id.clone()));
        assert_eq!(index1.cmp_prefix(&index2), std::cmp::Ordering::Less);
    }

    #[test]
    fn test_sub_id_from_str() {
        let sub_id = SubId::from("test_sub_id");
        assert_eq!(sub_id.0, "test_sub_id");
    }

    #[test]
    fn test_sub_id_deref() {
        let sub_id = SubId::from("test_sub_id");
        assert_eq!(&*sub_id, "test_sub_id");
    }
}

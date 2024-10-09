use super::SubId;
use std::{
    fmt::Debug,
    ops::Deref,
    sync::atomic::{AtomicUsize, Ordering},
};

/// Indexable trait
pub trait Indexable {
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
/// are the same, and also to make sure that ealier indexes matches first
pub struct Index<T>
where
    T: PartialOrd + Ord + Send + Sync + Debug,
{
    prefix: T,
    counter: Unique,
    id: super::SubId,
}

impl<T> Into<super::SubId> for &Index<T>
where
    T: PartialOrd + Ord + Send + Sync + Debug,
{
    fn into(self) -> super::SubId {
        self.id.clone()
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
            counter: Unique(0),
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
#[derive(Debug, Ord, PartialOrd, PartialEq, Eq, Clone)]
struct Unique(usize);

impl Default for Unique {
    fn default() -> Self {
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

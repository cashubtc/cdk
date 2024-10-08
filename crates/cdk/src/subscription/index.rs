use super::SubId;
use std::{
    ops::Deref,
    sync::atomic::{AtomicUsize, Ordering},
};

#[derive(Debug, Ord, PartialOrd, PartialEq, Eq, Clone)]
/// Index
///
/// The Index is a sorted structure that
pub struct Index<T>
where
    T: PartialOrd + Ord + Send + Sync,
{
    prefix: T,
    id: super::SubId,
    _unique: Unique,
}

impl<T> Deref for Index<T>
where
    T: PartialOrd + Ord + Send + Sync,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.prefix
    }
}

impl<T> Index<T>
where
    T: PartialOrd + Ord + Send + Sync,
{
    /// Compare the
    pub fn cmp_prefix(&self, other: &Index<T>) -> std::cmp::Ordering {
        self.prefix.cmp(&other.prefix)
    }
}

impl<T> From<(T, SubId)> for Index<T>
where
    T: PartialOrd + Ord + Send + Sync,
{
    fn from((prefix, id): (T, SubId)) -> Self {
        Self {
            prefix,
            id,
            _unique: Default::default(),
        }
    }
}

impl<T> From<T> for Index<T>
where
    T: PartialOrd + Ord + Send + Sync,
{
    fn from(prefix: T) -> Self {
        Self {
            prefix,
            id: Default::default(),
            _unique: Default::default(),
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
#[derive(Debug, Ord, PartialOrd, PartialEq, Eq, Clone)]
struct Unique(usize);

impl Default for Unique {
    fn default() -> Self {
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

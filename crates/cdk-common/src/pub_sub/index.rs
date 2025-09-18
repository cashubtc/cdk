//! WS Index

use std::fmt::Debug;
use std::hash::Hash;

use serde::de::DeserializeOwned;
use serde::Serialize;

/// Indexable trait
pub trait Indexable: Clone {
    /// Generic Index
    ///
    /// It should be serializable/deserializable to be stored in the database layer and it should
    /// also be sorted in a BTree for in-memory matching
    type Index: Debug
        + Clone
        + Eq
        + PartialEq
        + Ord
        + PartialOrd
        + Hash
        + Send
        + Sync
        + DeserializeOwned
        + Serialize;

    /// To indexes
    fn to_indexes(&self) -> Vec<Self::Index>;
}

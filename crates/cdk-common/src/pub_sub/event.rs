//! Pubsub Event definition
//!
//! The Pubsub Event defines the Topic struct and how an event can be converted to Topics.

use std::fmt::Debug;
use std::hash::Hash;

use serde::de::DeserializeOwned;
use serde::Serialize;

/// Indexable trait
pub trait Event: Clone {
    /// Generic Index
    ///
    /// It should be serializable/deserializable to be stored in the database layer and it should
    /// also be sorted in a BTree for in-memory matching
    type Topic: Debug
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
    fn get_topics(&self) -> Vec<Self::Topic>;
}

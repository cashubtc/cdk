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
use std::fmt::Debug;
use std::ops::Deref;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

pub mod index;

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

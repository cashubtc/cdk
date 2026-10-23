//! Publish/Subscribe core
//!
//! This module defines the transport-agnostic pub/sub primitives used by both
//! mint and wallet components. The design prioritizes:
//!
//! - **Request coalescing**: multiple local subscribers to the same remote topic
//!   result in a single upstream subscription, with local fan‑out.
//! - **Latest-on-subscribe** (NUT-17): on (re)subscription, the most recent event
//!   is fetched and delivered before streaming new ones.
//! - **Backpressure-aware delivery**: bounded channels + drop policies prevent
//!   a slow consumer from stalling the whole pipeline.
//! - **Resilience**: automatic reconnect with exponential backoff; WebSocket
//!   streaming when available, HTTP long-poll fallback otherwise.
//!
//! Terms used throughout the module:
//! - **Event**: a domain object that maps to one or more `Topic`s via `Event::get_topics`.
//! - **Topic**: an index/type that defines storage and matching semantics.
//! - **SubscriptionRequest**: a domain-specific filter that can be converted into
//!   low-level transport messages (e.g., WebSocket subscribe frames).
//! - **Spec**: type bundle tying `Event`, `Topic`, `SubscriptionId`, and serialization.

mod bus;
mod error;
mod pubsub;
pub mod remote_consumer;
mod subscriber;
#[cfg(any(test, feature = "test"))]
pub mod test;
mod types;

pub use self::bus::{Bus, LocalBus, LocalDelivery};
pub use self::error::Error;
pub use self::pubsub::Pubsub;
pub use self::subscriber::{Subscriber, SubscriptionRequest};
pub use self::types::*;

//! Pub-sub distribution bus
//!
//! The [`Bus`] trait abstracts *where* a published event goes before it reaches
//! this process's subscribers. The default [`LocalBus`] keeps everything
//! in-process, which is the only behavior the mint has today. A cross-process
//! bus (Redis pub/sub, Postgres `LISTEN/NOTIFY`, a message queue) can be added
//! as another `Bus` implementation without touching any publish call site.
//!
//! Local fan-out itself is unchanged: events are delivered to the in-memory
//! subscriber index over `tokio::sync::mpsc` channels. The bus only decides
//! whether a published event is also forwarded to peer instances, and injects
//! events received from peers back into the same local fan-out through
//! [`LocalDelivery`].

use super::pubsub::{Pubsub, TopicTree};
use super::Spec;
use crate::task::spawn;

/// Handle a [`Bus`] uses to deliver an event into this process's subscriber
/// set.
///
/// Cheap to clone (it wraps the shared subscriber index). Both locally
/// published events and events received from peer instances are delivered
/// through this same fan-out, so they follow one identical path.
#[allow(missing_debug_implementations)]
pub struct LocalDelivery<S>
where
    S: Spec + 'static,
{
    listeners_topics: TopicTree<S>,
}

impl<S> Clone for LocalDelivery<S>
where
    S: Spec + 'static,
{
    fn clone(&self) -> Self {
        Self {
            listeners_topics: self.listeners_topics.clone(),
        }
    }
}

impl<S> LocalDelivery<S>
where
    S: Spec + 'static,
{
    /// Create a new handle over the given subscriber index.
    pub(super) fn new(listeners_topics: TopicTree<S>) -> Self {
        Self { listeners_topics }
    }

    /// Fan an event out to matching in-process subscribers.
    pub fn deliver(&self, event: S::Event) {
        let _ = Pubsub::<S>::publish_internal(event, &self.listeners_topics);
    }
}

/// Distributes published events to subscribers.
///
/// The implementation decides the scope. [`LocalBus`] keeps everything
/// in-process (the current behavior); a Redis/Postgres bus additionally fans
/// out to, and in from, peer instances.
///
/// A bus is built with a [`LocalDelivery`] handle so it can deliver locally on
/// publish and, if it talks to peers, spawn its own inbound task that injects
/// peer events via the same handle.
pub trait Bus<S>: Send + Sync
where
    S: Spec + 'static,
{
    /// Deliver an event that originated on this instance.
    ///
    /// The implementation MUST deliver it to local subscribers and MAY forward
    /// it to peers. This is fire-and-forget, matching the [`Pubsub::publish`]
    /// contract: any network I/O runs on a spawned task.
    fn publish(&self, event: S::Event);
}

/// Default in-process bus.
///
/// Behavior is identical to publishing directly: deliver to local subscribers,
/// no peer fan-out.
#[allow(missing_debug_implementations)]
pub struct LocalBus<S>
where
    S: Spec + 'static,
{
    local: LocalDelivery<S>,
}

impl<S> LocalBus<S>
where
    S: Spec + 'static,
{
    /// Create a new in-process bus.
    pub fn new(local: LocalDelivery<S>) -> Self {
        Self { local }
    }
}

impl<S> Bus<S> for LocalBus<S>
where
    S: Spec + 'static,
{
    fn publish(&self, event: S::Event) {
        let local = self.local.clone();
        // Spawn preserves today's non-blocking publish semantics: the caller of
        // `Pubsub::publish` never waits on subscriber delivery.
        spawn(async move {
            local.deliver(event);
        });
    }
}

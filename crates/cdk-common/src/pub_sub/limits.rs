//! Ceilings on the shared resources held by a pub/sub instance.

/// Ceilings on the shared resources one [`Pubsub`](super::Pubsub) may hold.
///
/// These are per-process rather than per-subscriber: a public mint endpoint is
/// reached by many anonymous connections, so a per-connection cap alone is
/// multiplied by the number of sockets an attacker opens.
#[derive(Debug, Clone, Copy)]
pub struct PubsubLimits {
    /// Maximum number of topic registrations held by all live subscriptions.
    pub max_topics: usize,
    /// Maximum number of backfills running concurrently.
    pub max_concurrent_backfills: usize,
    /// Maximum number of quotes a single backfill may check against the payment
    /// backend. Bounds how long one backfill holds its concurrency slot.
    pub max_quote_checks_per_backfill: usize,
}

impl PubsubLimits {
    /// Topic budget used when none is configured.
    ///
    /// Unbounded because [`Pubsub`](super::Pubsub) also backs the wallet's local
    /// relay, where subscriptions are the wallet's own and a finite default would
    /// silently change its behaviour. A mint picks a finite budget explicitly.
    pub const DEFAULT_MAX_TOPICS: usize = usize::MAX;

    /// Concurrent backfills allowed when none is configured.
    pub const DEFAULT_MAX_CONCURRENT_BACKFILLS: usize = 32;

    /// Payment-backend checks per backfill allowed when none is configured.
    pub const DEFAULT_MAX_QUOTE_CHECKS_PER_BACKFILL: usize = 64;
}

impl Default for PubsubLimits {
    fn default() -> Self {
        Self {
            max_topics: Self::DEFAULT_MAX_TOPICS,
            max_concurrent_backfills: Self::DEFAULT_MAX_CONCURRENT_BACKFILLS,
            max_quote_checks_per_backfill: Self::DEFAULT_MAX_QUOTE_CHECKS_PER_BACKFILL,
        }
    }
}

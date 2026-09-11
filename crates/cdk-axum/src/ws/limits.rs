//! Ceilings applied to the public WebSocket endpoint.

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Operator-tunable ceilings for the public `/v1/ws` endpoint.
#[derive(Debug, Clone, Copy)]
pub struct WsLimits {
    /// Maximum concurrent WebSocket connections across the whole process.
    pub max_connections: usize,
    /// Maximum concurrent WebSocket connections from a single peer address.
    ///
    /// `0` disables the per-address cap, which is what a mint behind a reverse
    /// proxy needs, since every connection then arrives from the proxy.
    pub max_connections_per_ip: usize,
    /// Maximum concurrent subscriptions on one connection.
    pub max_subscriptions_per_connection: usize,
    /// Maximum filters accepted in a single subscription request.
    pub max_filters_per_subscription: usize,
    /// Maximum topics one connection may register across all its subscriptions.
    pub max_topics_per_connection: usize,
    /// Sustained rate at which one connection earns request budget, in units
    /// per second.
    ///
    /// `0` disables the throttle. A unit is roughly one frame; a subscription
    /// costs one unit per filter, because registering a filter is what takes the
    /// mint-wide topic lock.
    pub max_request_units_per_second: u32,
    /// Request budget one connection may hold unspent, in units.
    ///
    /// Must cover a maximum-size subscription, or such a request could never
    /// succeed.
    pub max_request_burst_units: u32,
    /// How many throttled requests a connection may make before it is closed.
    ///
    /// `0` keeps a throttled connection open indefinitely. Answering a
    /// throttled frame still costs a socket read, so a client that keeps
    /// flooding after being refused is cheaper to drop than to answer.
    pub max_throttled_requests: usize,
    /// Largest WebSocket message accepted from a client, in bytes.
    pub max_message_bytes: usize,
    /// How long a connection may go without inbound traffic before it is closed.
    pub idle_timeout: Duration,
    /// How often an otherwise silent connection is pinged.
    pub ping_interval: Duration,
}

impl WsLimits {
    /// Concurrent connections allowed when none is configured.
    pub const DEFAULT_MAX_CONNECTIONS: usize = 512;

    /// Concurrent connections per peer address allowed when none is configured.
    pub const DEFAULT_MAX_CONNECTIONS_PER_IP: usize = 2;

    /// Subscriptions per connection allowed when none is configured.
    ///
    /// A rejected subscription tears down the whole stream on the reference
    /// wallet, so the default stays above what a busy client plausibly opens
    /// and the mint-wide budgets do the real limiting.
    pub const DEFAULT_MAX_SUBSCRIPTIONS_PER_CONNECTION: usize = 100;

    /// Filters per subscription allowed when none is configured.
    pub const DEFAULT_MAX_FILTERS_PER_SUBSCRIPTION: usize = 1000;

    /// Topics per connection allowed when none is configured.
    pub const DEFAULT_MAX_TOPICS_PER_CONNECTION: usize = 1000;

    /// Sustained request rate allowed when none is configured.
    pub const DEFAULT_MAX_REQUEST_UNITS_PER_SECOND: u32 = 32;

    /// Unspent request budget allowed when none is configured.
    ///
    /// Comfortably above `1 + DEFAULT_MAX_FILTERS_PER_SUBSCRIPTION`, so a
    /// maximum-size subscription fits with room left for ordinary traffic.
    pub const DEFAULT_MAX_REQUEST_BURST_UNITS: u32 = 1024;

    /// Throttled requests tolerated before closing, when none is configured.
    pub const DEFAULT_MAX_THROTTLED_REQUESTS: usize = 20;

    /// Largest accepted message when none is configured.
    pub const DEFAULT_MAX_MESSAGE_BYTES: usize = 512 * 1024;

    /// Idle timeout applied when none is configured.
    pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

    /// Keepalive interval applied when none is configured.
    pub const DEFAULT_PING_INTERVAL: Duration = Duration::from_secs(30);
}

impl Default for WsLimits {
    fn default() -> Self {
        Self {
            max_connections: Self::DEFAULT_MAX_CONNECTIONS,
            max_connections_per_ip: Self::DEFAULT_MAX_CONNECTIONS_PER_IP,
            max_subscriptions_per_connection: Self::DEFAULT_MAX_SUBSCRIPTIONS_PER_CONNECTION,
            max_filters_per_subscription: Self::DEFAULT_MAX_FILTERS_PER_SUBSCRIPTION,
            max_topics_per_connection: Self::DEFAULT_MAX_TOPICS_PER_CONNECTION,
            max_request_units_per_second: Self::DEFAULT_MAX_REQUEST_UNITS_PER_SECOND,
            max_request_burst_units: Self::DEFAULT_MAX_REQUEST_BURST_UNITS,
            max_throttled_requests: Self::DEFAULT_MAX_THROTTLED_REQUESTS,
            max_message_bytes: Self::DEFAULT_MAX_MESSAGE_BYTES,
            idle_timeout: Self::DEFAULT_IDLE_TIMEOUT,
            ping_interval: Self::DEFAULT_PING_INTERVAL,
        }
    }
}

type PerIpCounts = Arc<Mutex<HashMap<IpAddr, usize>>>;

/// Why a WebSocket upgrade was refused.
#[derive(Debug, Clone, Copy)]
pub(crate) enum WsRejection {
    /// The process-wide connection budget is fully in use.
    ServerFull,
    /// The peer address already holds its allowance of connections.
    PerIpFull,
    /// The limiter could not be consulted, so the upgrade fails closed.
    Unavailable,
}

/// Tracks how much of the connection budget is in use.
#[derive(Debug)]
pub(crate) struct WsConnectionLimiter {
    limits: WsLimits,
    global: Arc<Semaphore>,
    per_ip: PerIpCounts,
}

impl WsConnectionLimiter {
    pub(crate) fn new(limits: WsLimits) -> Self {
        Self {
            global: Arc::new(Semaphore::new(limits.max_connections)),
            per_ip: Arc::new(Mutex::new(HashMap::new())),
            limits,
        }
    }

    pub(crate) fn limits(&self) -> WsLimits {
        self.limits
    }

    /// Claim a connection slot, or say why one is not available.
    ///
    /// `peer` is `None` when the router is served without connect info, in which
    /// case only the process-wide budget applies.
    pub(crate) fn try_acquire(
        &self,
        peer: Option<IpAddr>,
    ) -> Result<WsConnectionGuard, WsRejection> {
        // Taken first and held in a local, so every rejection below returns it
        // to the semaphore on the way out without an explicit rollback.
        let global = self
            .global
            .clone()
            .try_acquire_owned()
            .map_err(|_| WsRejection::ServerFull)?;

        let per_ip = match (peer, self.limits.max_connections_per_ip) {
            (Some(ip), max) if max > 0 => {
                let mut counts = self.per_ip.lock().map_err(|err| {
                    // Fail closed: a cap that cannot be enforced is worse than a
                    // refused connection.
                    tracing::warn!("WebSocket per-IP table poisoned, refusing upgrade: {err}");
                    WsRejection::Unavailable
                })?;

                let slot = counts.entry(ip).or_insert(0);
                if *slot >= max {
                    return Err(WsRejection::PerIpFull);
                }
                *slot += 1;

                Some((self.per_ip.clone(), ip))
            }
            _ => None,
        };

        Ok(WsConnectionGuard {
            _global: global,
            per_ip,
        })
    }

    #[cfg(test)]
    pub(crate) fn tracked_addresses(&self) -> usize {
        self.per_ip.lock().map(|counts| counts.len()).unwrap_or(0)
    }
}

/// Holds a connection's slots for the lifetime of the socket.
///
/// Releasing on drop rather than at the end of the read loop covers abrupt
/// disconnects and panics, which is when slots would otherwise leak.
#[derive(Debug)]
pub(crate) struct WsConnectionGuard {
    _global: OwnedSemaphorePermit,
    per_ip: Option<(PerIpCounts, IpAddr)>,
}

impl Drop for WsConnectionGuard {
    fn drop(&mut self) {
        let Some((counts, ip)) = self.per_ip.take() else {
            return;
        };

        let locked = counts.lock();
        match locked {
            Ok(mut counts) => {
                if let Entry::Occupied(mut entry) = counts.entry(ip) {
                    *entry.get_mut() -= 1;
                    // Removed at zero so the table stays bounded by the
                    // process-wide connection budget.
                    if *entry.get() == 0 {
                        entry.remove();
                    }
                }
            }
            // Logged rather than ignored: the slot stays claimed until restart,
            // permanently shrinking that address's allowance.
            Err(err) => tracing::warn!("WebSocket per-IP table poisoned on release: {err}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    fn limiter(max_connections: usize, max_connections_per_ip: usize) -> WsConnectionLimiter {
        WsConnectionLimiter::new(WsLimits {
            max_connections,
            max_connections_per_ip,
            ..WsLimits::default()
        })
    }

    fn ip(last: u8) -> Option<IpAddr> {
        Some(IpAddr::V4(Ipv4Addr::new(127, 0, 0, last)))
    }

    #[test]
    fn global_budget_is_enforced() {
        let limiter = limiter(2, 0);

        let _first = limiter.try_acquire(ip(1)).expect("first connection");
        let _second = limiter.try_acquire(ip(2)).expect("second connection");

        assert!(matches!(
            limiter.try_acquire(ip(3)),
            Err(WsRejection::ServerFull)
        ));
    }

    #[test]
    fn per_ip_budget_is_enforced() {
        let limiter = limiter(10, 2);

        let _first = limiter.try_acquire(ip(1)).expect("first connection");
        let _second = limiter.try_acquire(ip(1)).expect("second connection");

        assert!(matches!(
            limiter.try_acquire(ip(1)),
            Err(WsRejection::PerIpFull)
        ));
        limiter
            .try_acquire(ip(2))
            .expect("a different address still has its own allowance");
    }

    /// A per-address rejection must hand back the global permit it took first,
    /// or one busy client would drain the process-wide budget.
    #[test]
    fn per_ip_rejection_releases_the_global_permit() {
        let limiter = limiter(4, 1);

        let _held = limiter.try_acquire(ip(1)).expect("first connection");
        assert!(matches!(
            limiter.try_acquire(ip(1)),
            Err(WsRejection::PerIpFull)
        ));

        assert_eq!(limiter.global.available_permits(), 3);
    }

    #[test]
    fn dropping_a_guard_releases_both_slots() {
        let limiter = limiter(1, 1);

        let guard = limiter.try_acquire(ip(1)).expect("first connection");
        assert!(matches!(
            limiter.try_acquire(ip(1)),
            Err(WsRejection::ServerFull)
        ));

        drop(guard);

        assert_eq!(limiter.tracked_addresses(), 0);
        limiter.try_acquire(ip(1)).expect("slot is available again");
    }

    #[test]
    fn zero_per_ip_limit_only_applies_the_global_budget() {
        let limiter = limiter(3, 0);

        let _first = limiter.try_acquire(ip(1)).expect("first connection");
        let _second = limiter.try_acquire(ip(1)).expect("second connection");
        let _third = limiter.try_acquire(ip(1)).expect("third connection");

        assert_eq!(limiter.tracked_addresses(), 0);
        assert!(matches!(
            limiter.try_acquire(ip(1)),
            Err(WsRejection::ServerFull)
        ));
    }

    #[test]
    fn a_missing_peer_address_skips_the_per_ip_budget() {
        let limiter = limiter(3, 1);

        let _first = limiter.try_acquire(None).expect("first connection");
        let _second = limiter.try_acquire(None).expect("second connection");

        assert_eq!(limiter.tracked_addresses(), 0);
    }
}

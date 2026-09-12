//! Ceilings applied to the public WebSocket endpoint.

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fmt;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::HeaderName;
use thiserror::Error;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Operator-tunable ceilings for the public `/v1/ws` endpoint, and how a client
/// is identified for the ones that are per-client.
#[derive(Debug, Clone)]
pub struct WsLimits {
    /// Maximum concurrent WebSocket connections across the whole process.
    pub max_connections: usize,
    /// Maximum concurrent WebSocket connections from a single client address.
    ///
    /// `0` disables the per-address cap, which is what a mint behind a reverse
    /// proxy needs unless `trusted_client_ip_header` tells it which header
    /// carries the real client, since every connection otherwise arrives from
    /// the proxy.
    pub max_connections_per_ip: usize,
    /// Header a trusted reverse proxy sets with the client's address, read in
    /// place of the TCP peer by the per-address cap.
    ///
    /// `None` keys the cap on the peer address. Only set this when a proxy in
    /// front of the mint overwrites or appends to the header, because any client
    /// can send it otherwise and the cap becomes trivial to evade.
    pub trusted_client_ip_header: Option<HeaderName>,
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

    /// Concurrent connections per client address allowed when none is configured.
    ///
    /// Disabled, because the usual deployment terminates TLS at a proxy and every
    /// connection then arrives from one address, where the cap would apply to all
    /// clients at once until `trusted_client_ip_header` is set. `max_connections`
    /// and the per-connection request and subscription budgets already bound an
    /// anonymous client.
    pub const DEFAULT_MAX_CONNECTIONS_PER_IP: usize = 0;

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

    /// Smallest accepted `max_message_bytes`, below which a legitimate subscribe
    /// frame would no longer fit.
    pub const MIN_MAX_MESSAGE_BYTES: usize = 4096;

    /// Rejects limits the endpoint could not serve.
    ///
    /// Checked before a router is built rather than when traffic arrives, because
    /// these failures are silent or fatal: a zero `ping_interval` panics the
    /// connection task, a zero `idle_timeout` makes every write time out at once,
    /// and a budget too small for one maximum-size subscription refuses that
    /// request for the life of the process.
    pub fn validate(&self) -> Result<(), WsLimitsError> {
        for (field, value) in [
            (WsLimitsField::MaxConnections, self.max_connections),
            (
                WsLimitsField::MaxSubscriptionsPerConnection,
                self.max_subscriptions_per_connection,
            ),
            (
                WsLimitsField::MaxFiltersPerSubscription,
                self.max_filters_per_subscription,
            ),
            (
                WsLimitsField::MaxTopicsPerConnection,
                self.max_topics_per_connection,
            ),
        ] {
            if value == 0 {
                return Err(WsLimitsError::Zero { field });
            }
        }

        if self.max_connections > Semaphore::MAX_PERMITS {
            return Err(WsLimitsError::TooManyConnections {
                maximum: Semaphore::MAX_PERMITS,
            });
        }

        if self.max_request_units_per_second > 0 {
            let required = u32::try_from(self.max_filters_per_subscription)
                .unwrap_or(u32::MAX)
                .saturating_add(1);
            if self.max_request_burst_units < required {
                return Err(WsLimitsError::BurstTooSmall { required });
            }
        }

        if self.max_topics_per_connection < self.max_filters_per_subscription {
            return Err(WsLimitsError::TopicBudgetTooSmall {
                required: self.max_filters_per_subscription,
            });
        }

        if self.max_message_bytes < Self::MIN_MAX_MESSAGE_BYTES {
            return Err(WsLimitsError::MessageTooSmall {
                required: Self::MIN_MAX_MESSAGE_BYTES,
            });
        }

        if self.idle_timeout.is_zero() {
            return Err(WsLimitsError::Zero {
                field: WsLimitsField::IdleTimeout,
            });
        }

        if self.ping_interval.is_zero() {
            return Err(WsLimitsError::Zero {
                field: WsLimitsField::PingInterval,
            });
        }

        if self.ping_interval > self.idle_timeout {
            return Err(WsLimitsError::PingIntervalExceedsIdleTimeout);
        }

        Ok(())
    }
}

/// A [`WsLimits`] field an invalid configuration can be blamed on, so a caller
/// that exposes these limits under its own names can report the failure in its
/// own vocabulary.
///
/// Only fields [`WsLimits::validate`] can actually blame are named, because a
/// variant that never occurs forces callers to write an arm that never runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WsLimitsField {
    /// [`WsLimits::max_connections`].
    MaxConnections,
    /// [`WsLimits::max_subscriptions_per_connection`].
    MaxSubscriptionsPerConnection,
    /// [`WsLimits::max_filters_per_subscription`].
    MaxFiltersPerSubscription,
    /// [`WsLimits::max_topics_per_connection`].
    MaxTopicsPerConnection,
    /// [`WsLimits::max_request_burst_units`].
    MaxRequestBurstUnits,
    /// [`WsLimits::max_message_bytes`].
    MaxMessageBytes,
    /// [`WsLimits::idle_timeout`].
    IdleTimeout,
    /// [`WsLimits::ping_interval`].
    PingInterval,
}

impl fmt::Display for WsLimitsField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::MaxConnections => "max_connections",
            Self::MaxSubscriptionsPerConnection => "max_subscriptions_per_connection",
            Self::MaxFiltersPerSubscription => "max_filters_per_subscription",
            Self::MaxTopicsPerConnection => "max_topics_per_connection",
            Self::MaxRequestBurstUnits => "max_request_burst_units",
            Self::MaxMessageBytes => "max_message_bytes",
            Self::IdleTimeout => "idle_timeout",
            Self::PingInterval => "ping_interval",
        };
        f.write_str(name)
    }
}

/// Why a [`WsLimits`] set cannot be served.
///
/// Each variant carries the value an operator has to reach, so a caller can
/// restate the failure without recomputing the rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum WsLimitsError {
    /// A limit that has to leave at least one request possible was zero.
    #[error("{field} must be greater than zero")]
    Zero {
        /// The limit that was zero.
        field: WsLimitsField,
    },
    /// The request budget could not cover one maximum-size subscription, which
    /// refuses every such request for the life of the process.
    #[error("max_request_burst_units must be at least {required} to cover the frame and every filter of one max_filters_per_subscription request")]
    BurstTooSmall {
        /// Units one maximum-size subscription costs.
        required: u32,
    },
    /// The per-connection topic budget could not cover one maximum-size
    /// subscription, which is a permanent refusal dressed up as a rate limit.
    #[error("max_topics_per_connection must be at least {required} to admit one max_filters_per_subscription request")]
    TopicBudgetTooSmall {
        /// Topics one maximum-size subscription registers.
        required: usize,
    },
    /// The message ceiling was below what a legitimate subscribe frame needs.
    #[error("max_message_bytes must be at least {required}")]
    MessageTooSmall {
        /// Smallest ceiling that still admits a legitimate request.
        required: usize,
    },
    /// A ping tick is the only moment the idle check runs, so pings arriving
    /// after the timeout had elapsed would stretch it silently.
    #[error("ping_interval must not exceed idle_timeout")]
    PingIntervalExceedsIdleTimeout,
    /// More connections than the semaphore backing the global budget accepts.
    #[error("max_connections must not exceed {maximum}")]
    TooManyConnections {
        /// Largest budget the connection semaphore accepts.
        maximum: usize,
    },
}

impl WsLimitsError {
    /// The field an operator has to change, so a caller can name it in its own
    /// configuration vocabulary without matching on every variant.
    pub const fn field(self) -> WsLimitsField {
        match self {
            Self::Zero { field } => field,
            Self::BurstTooSmall { .. } => WsLimitsField::MaxRequestBurstUnits,
            Self::TopicBudgetTooSmall { .. } => WsLimitsField::MaxTopicsPerConnection,
            Self::MessageTooSmall { .. } => WsLimitsField::MaxMessageBytes,
            Self::PingIntervalExceedsIdleTimeout => WsLimitsField::PingInterval,
            Self::TooManyConnections { .. } => WsLimitsField::MaxConnections,
        }
    }
}

impl Default for WsLimits {
    fn default() -> Self {
        Self {
            max_connections: Self::DEFAULT_MAX_CONNECTIONS,
            max_connections_per_ip: Self::DEFAULT_MAX_CONNECTIONS_PER_IP,
            trusted_client_ip_header: None,
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

    pub(crate) fn limits(&self) -> &WsLimits {
        &self.limits
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

    #[test]
    fn the_shipped_defaults_validate() {
        WsLimits::default()
            .validate()
            .expect("the shipped defaults must be servable");
    }

    /// The limits documented as "0 disables it" must survive the nonzero rules,
    /// or a mint behind a reverse proxy could not start.
    #[test]
    fn the_optional_limits_may_be_zero() {
        WsLimits {
            max_connections_per_ip: 0,
            max_request_units_per_second: 0,
            max_request_burst_units: 0,
            max_throttled_requests: 0,
            ..WsLimits::default()
        }
        .validate()
        .expect("zero disables these limits rather than breaking them");
    }

    #[test]
    fn a_limit_that_refuses_every_request_is_rejected() {
        for (limits, field) in [
            (
                WsLimits {
                    max_connections: 0,
                    ..WsLimits::default()
                },
                WsLimitsField::MaxConnections,
            ),
            (
                WsLimits {
                    max_subscriptions_per_connection: 0,
                    ..WsLimits::default()
                },
                WsLimitsField::MaxSubscriptionsPerConnection,
            ),
            (
                WsLimits {
                    max_filters_per_subscription: 0,
                    ..WsLimits::default()
                },
                WsLimitsField::MaxFiltersPerSubscription,
            ),
            (
                WsLimits {
                    max_topics_per_connection: 0,
                    ..WsLimits::default()
                },
                WsLimitsField::MaxTopicsPerConnection,
            ),
            (
                WsLimits {
                    idle_timeout: Duration::ZERO,
                    ..WsLimits::default()
                },
                WsLimitsField::IdleTimeout,
            ),
            (
                WsLimits {
                    ping_interval: Duration::ZERO,
                    ..WsLimits::default()
                },
                WsLimitsField::PingInterval,
            ),
        ] {
            assert_eq!(
                limits.validate(),
                Err(WsLimitsError::Zero { field }),
                "{field} must be rejected at zero"
            );
        }
    }

    #[test]
    fn a_burst_smaller_than_one_subscription_is_rejected() {
        let limits = WsLimits {
            max_filters_per_subscription: 100,
            max_topics_per_connection: 100,
            max_request_units_per_second: 32,
            max_request_burst_units: 100,
            ..WsLimits::default()
        };

        assert_eq!(
            limits.validate(),
            Err(WsLimitsError::BurstTooSmall { required: 101 })
        );
        assert_eq!(
            WsLimits {
                max_request_burst_units: 101,
                ..limits
            }
            .validate(),
            Ok(())
        );
    }

    #[test]
    fn a_topic_budget_smaller_than_one_subscription_is_rejected() {
        let limits = WsLimits {
            max_filters_per_subscription: 10,
            max_topics_per_connection: 9,
            ..WsLimits::default()
        };

        assert_eq!(
            limits.validate(),
            Err(WsLimitsError::TopicBudgetTooSmall { required: 10 })
        );
    }

    #[test]
    fn a_message_ceiling_below_the_floor_is_rejected() {
        for bytes in [0, WsLimits::MIN_MAX_MESSAGE_BYTES - 1] {
            assert_eq!(
                WsLimits {
                    max_message_bytes: bytes,
                    ..WsLimits::default()
                }
                .validate(),
                Err(WsLimitsError::MessageTooSmall {
                    required: WsLimits::MIN_MAX_MESSAGE_BYTES
                })
            );
        }
    }

    /// A ping is the only thing that runs the idle check, so a ping interval past
    /// the timeout stretches it silently.
    #[test]
    fn a_ping_interval_past_the_idle_timeout_is_rejected() {
        let limits = WsLimits {
            idle_timeout: Duration::from_secs(30),
            ping_interval: Duration::from_secs(31),
            ..WsLimits::default()
        };

        assert_eq!(
            limits.validate(),
            Err(WsLimitsError::PingIntervalExceedsIdleTimeout)
        );
        assert_eq!(
            WsLimits {
                ping_interval: Duration::from_secs(30),
                ..limits
            }
            .validate(),
            Ok(())
        );
    }

    /// The connection semaphore panics above its permit ceiling, so the router has
    /// to refuse the configuration first.
    #[test]
    fn a_connection_budget_the_semaphore_cannot_hold_is_rejected() {
        assert_eq!(
            WsLimits {
                max_connections: usize::MAX,
                ..WsLimits::default()
            }
            .validate(),
            Err(WsLimitsError::TooManyConnections {
                maximum: Semaphore::MAX_PERMITS
            })
        );
        WsLimits {
            max_connections: Semaphore::MAX_PERMITS,
            ..WsLimits::default()
        }
        .validate()
        .expect("the ceiling itself is servable");
    }
}

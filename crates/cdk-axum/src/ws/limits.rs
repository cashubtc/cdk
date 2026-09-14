//! Ceilings applied to the public WebSocket endpoint.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Operator-tunable ceilings for the public `/v1/ws` endpoint.
#[derive(Debug, Clone)]
pub struct WsLimits {
    /// Maximum concurrent WebSocket connections across the whole process.
    pub max_connections: usize,
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
    ///
    /// Must be shorter than `idle_timeout`: the idle check runs on the same
    /// tick that sends the ping, so a quiet connection has only the difference
    /// between the two in which to answer.
    pub ping_interval: Duration,
}

impl WsLimits {
    /// Concurrent connections allowed when none is configured.
    pub const DEFAULT_MAX_CONNECTIONS: usize = 512;

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

        if self.ping_interval >= self.idle_timeout {
            return Err(WsLimitsError::PingIntervalNotShorterThanIdleTimeout);
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
    #[error("max_request_burst_units must be at least {required}: one unit for the frame and one for each filter of the largest subscription")]
    BurstTooSmall {
        /// Units one maximum-size subscription costs.
        required: u32,
    },
    /// The per-connection topic budget could not cover one maximum-size
    /// subscription, which is a permanent refusal dressed up as a rate limit.
    #[error("max_topics_per_connection must be at least {required}, one topic for each filter of the largest subscription")]
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
    /// A ping tick is the only moment the idle check runs, and the check comes
    /// first, so at equal values a quiet connection is closed on the tick that
    /// would have pinged it. The difference between the two is the window a
    /// client has to answer.
    #[error("ping_interval must be shorter than the idle timeout, which is the window a client has to answer a ping")]
    PingIntervalNotShorterThanIdleTimeout,
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
            Self::PingIntervalNotShorterThanIdleTimeout => WsLimitsField::PingInterval,
            Self::TooManyConnections { .. } => WsLimitsField::MaxConnections,
        }
    }
}

impl Default for WsLimits {
    fn default() -> Self {
        Self {
            max_connections: Self::DEFAULT_MAX_CONNECTIONS,
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

/// Tracks how much of the connection budget is in use.
#[derive(Debug)]
pub(crate) struct WsConnectionLimiter {
    limits: WsLimits,
    global: Arc<Semaphore>,
}

impl WsConnectionLimiter {
    pub(crate) fn new(limits: WsLimits) -> Self {
        Self {
            global: Arc::new(Semaphore::new(limits.max_connections)),
            limits,
        }
    }

    pub(crate) fn limits(&self) -> &WsLimits {
        &self.limits
    }

    /// Claim a connection slot, or `None` when the process-wide budget is fully
    /// in use.
    pub(crate) fn try_acquire(&self) -> Option<WsConnectionGuard> {
        self.global
            .clone()
            .try_acquire_owned()
            .ok()
            .map(|permit| WsConnectionGuard { _permit: permit })
    }
}

/// Holds a connection's slot for the lifetime of the socket.
///
/// Releasing on drop rather than at the end of the read loop covers abrupt
/// disconnects and panics, which is when the slot would otherwise leak.
#[derive(Debug)]
pub(crate) struct WsConnectionGuard {
    _permit: OwnedSemaphorePermit,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limiter(max_connections: usize) -> WsConnectionLimiter {
        WsConnectionLimiter::new(WsLimits {
            max_connections,
            ..WsLimits::default()
        })
    }

    #[test]
    fn global_budget_is_enforced() {
        let limiter = limiter(2);

        let _first = limiter.try_acquire().expect("first connection");
        let _second = limiter.try_acquire().expect("second connection");

        assert!(limiter.try_acquire().is_none());
    }

    #[test]
    fn dropping_a_guard_releases_the_slot() {
        let limiter = limiter(1);

        let guard = limiter.try_acquire().expect("first connection");
        assert!(limiter.try_acquire().is_none());

        drop(guard);

        limiter.try_acquire().expect("slot is available again");
    }

    #[test]
    fn the_shipped_defaults_validate() {
        WsLimits::default()
            .validate()
            .expect("the shipped defaults must be servable");
    }

    /// The limits documented as "0 disables it" must survive the nonzero rules,
    /// or an operator turning the throttle off could not start the mint.
    #[test]
    fn the_optional_limits_may_be_zero() {
        WsLimits {
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

    /// The idle check runs on the ping tick and runs first, so at equal values
    /// a quiet connection is closed on the tick that would have pinged it: it
    /// never gets a ping to answer.
    #[test]
    fn a_ping_interval_no_shorter_than_the_idle_timeout_is_rejected() {
        let limits = WsLimits {
            idle_timeout: Duration::from_secs(30),
            ping_interval: Duration::from_secs(31),
            ..WsLimits::default()
        };

        for ping in [Duration::from_secs(31), Duration::from_secs(30)] {
            assert_eq!(
                WsLimits {
                    ping_interval: ping,
                    ..limits
                }
                .validate(),
                Err(WsLimitsError::PingIntervalNotShorterThanIdleTimeout),
                "a {ping:?} ping under a 30s idle timeout leaves no window to answer"
            );
        }

        assert_eq!(
            WsLimits {
                ping_interval: Duration::from_secs(29),
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

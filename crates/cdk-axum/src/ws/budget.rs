//! Per-connection budget for inbound WebSocket requests.

use std::time::{Duration, Instant};

use super::WsLimits;

/// Outcome of charging a connection's request budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Charge {
    /// The budget covered the request.
    Accepted,
    /// The budget is spent; answer `ServerBusy` and keep the socket open.
    Throttled,
    /// The connection has been throttled too often; close it.
    Exhausted,
}

/// A per-connection token bucket charged in units of work rather than in
/// frames, so a subscription registering a hundred topics costs a hundred times
/// what a one-topic subscription costs.
///
/// The clock is a parameter rather than read internally, so the refill can be
/// tested without a sleep.
#[derive(Debug)]
pub(crate) struct RequestBudget {
    capacity: u32,
    units: u32,
    /// `None` when the operator disabled the throttle, or when the configured
    /// rate is so high that one unit takes less than a nanosecond to earn.
    unit_interval: Option<Duration>,
    refilled_at: Instant,
    throttled: usize,
    max_throttled: usize,
}

impl RequestBudget {
    /// Builds a full budget from the connection's limits.
    pub(crate) fn new(limits: &WsLimits, now: Instant) -> Self {
        let capacity = limits.max_request_burst_units.max(1);
        let unit_interval = Duration::from_secs(1)
            .checked_div(limits.max_request_units_per_second)
            .filter(|interval| !interval.is_zero());

        Self {
            capacity,
            units: capacity,
            unit_interval,
            refilled_at: now,
            throttled: 0,
            max_throttled: limits.max_throttled_requests,
        }
    }

    /// Charges `units` of work against the budget.
    ///
    /// A client that pauses long enough to refill completely is forgiven its
    /// earlier strikes, so a long-lived honest connection never accumulates them
    /// across days.
    pub(crate) fn charge(&mut self, units: u32, now: Instant) -> Charge {
        let Some(interval) = self.unit_interval else {
            return Charge::Accepted;
        };

        self.refill(interval, now);

        if self.units == self.capacity {
            self.throttled = 0;
        }

        if units > self.units {
            self.throttled += 1;
            if self.max_throttled > 0 && self.throttled >= self.max_throttled {
                return Charge::Exhausted;
            }
            return Charge::Throttled;
        }

        self.units -= units;
        Charge::Accepted
    }

    /// Credits whole units earned since the last refill.
    ///
    /// The remainder is kept by advancing `refilled_at` by exactly what was
    /// credited rather than to `now`, so a stream of sub-interval charges cannot
    /// round the earned fraction away and stall the refill.
    fn refill(&mut self, interval: Duration, now: Instant) {
        let elapsed = now.saturating_duration_since(self.refilled_at);
        let earned = elapsed.as_nanos() / interval.as_nanos();
        if earned == 0 {
            return;
        }

        let missing = u128::from(self.capacity - self.units);
        if earned >= missing {
            self.units = self.capacity;
            self.refilled_at = now;
        } else {
            let earned = earned as u32;
            self.units += earned;
            self.refilled_at += interval.saturating_mul(earned);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(rate: u32, burst: u32, max_throttled: usize) -> WsLimits {
        WsLimits {
            max_request_units_per_second: rate,
            max_request_burst_units: burst,
            max_throttled_requests: max_throttled,
            ..WsLimits::default()
        }
    }

    #[test]
    fn burst_is_spent_then_refused() {
        let now = Instant::now();
        let mut budget = RequestBudget::new(&limits(1, 3, 0), now);

        for _ in 0..3 {
            assert_eq!(budget.charge(1, now), Charge::Accepted);
        }
        assert_eq!(budget.charge(1, now), Charge::Throttled);
    }

    #[test]
    fn units_refill_over_time() {
        let start = Instant::now();
        let mut budget = RequestBudget::new(&limits(4, 4, 0), start);

        for _ in 0..4 {
            assert_eq!(budget.charge(1, start), Charge::Accepted);
        }
        assert_eq!(budget.charge(1, start), Charge::Throttled);

        let later = start + Duration::from_millis(250);
        assert_eq!(budget.charge(1, later), Charge::Accepted);
        assert_eq!(budget.charge(1, later), Charge::Throttled);
    }

    #[test]
    fn a_full_bucket_forgives_earlier_throttling() {
        let start = Instant::now();
        let mut budget = RequestBudget::new(&limits(4, 2, 3), start);

        assert_eq!(budget.charge(2, start), Charge::Accepted);
        assert_eq!(budget.charge(1, start), Charge::Throttled);
        assert_eq!(budget.charge(1, start), Charge::Throttled);

        let recovered = start + Duration::from_secs(1);
        assert_eq!(budget.charge(1, recovered), Charge::Accepted);

        assert_eq!(budget.charge(2, recovered), Charge::Throttled);
        assert_eq!(budget.charge(2, recovered), Charge::Throttled);
    }

    #[test]
    fn repeated_throttling_exhausts_the_connection() {
        let now = Instant::now();
        let mut budget = RequestBudget::new(&limits(1, 1, 2), now);

        assert_eq!(budget.charge(1, now), Charge::Accepted);
        assert_eq!(budget.charge(1, now), Charge::Throttled);
        assert_eq!(budget.charge(1, now), Charge::Exhausted);
    }

    #[test]
    fn a_zero_rate_disables_the_budget() {
        let now = Instant::now();
        let mut budget = RequestBudget::new(&limits(0, 1, 1), now);

        for _ in 0..1000 {
            assert_eq!(budget.charge(u32::MAX, now), Charge::Accepted);
        }
    }

    /// A charge larger than what is left must not partially drain the bucket, or
    /// a client asking for too much would starve its own smaller requests.
    #[test]
    fn an_oversized_charge_leaves_the_remainder_intact() {
        let now = Instant::now();
        let mut budget = RequestBudget::new(&limits(1, 4, 0), now);

        assert_eq!(budget.charge(3, now), Charge::Accepted);
        assert_eq!(budget.charge(2, now), Charge::Throttled);
        assert_eq!(budget.charge(1, now), Charge::Accepted);
    }
}

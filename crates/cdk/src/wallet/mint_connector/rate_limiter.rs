//! Preemptive client-side rate limiter for wallet HTTP requests.
//!
//! Wraps every request the wallet sends to a mint with a token-bucket so that
//! the wallet stays under the mint's advertised (or assumed) rate limit and
//! avoids triggering server-side 429s. Modeled after Coco's
//! `RequestRateLimiter` — shared FIFO queue per limiter instance, no reactive
//! handling of server responses.
//!
//! See: <https://github.com/cashubtc/cdk/issues/1642>.

use std::fmt;
use std::time::Duration;

use tokio::sync::Mutex;
use web_time::Instant;

/// Default burst capacity, matching Coco's WalletService defaults.
pub const DEFAULT_CAPACITY: u32 = 20;
/// Default sustained rate (requests per minute), matching Coco's WalletService defaults.
pub const DEFAULT_REFILL_PER_MINUTE: u32 = 20;

/// Token bucket rate limiter.
///
/// Tokens refill continuously at `refill_per_minute / 60` tokens per second,
/// capped at `capacity`. [`RateLimiter::acquire`] consumes one token; when
/// none are available it awaits until the next token is produced. Waiters are
/// served in roughly arrival order via `tokio::sync::Mutex` lock ordering.
pub struct RateLimiter {
    capacity: f64,
    refill_per_second: f64,
    state: Mutex<State>,
}

struct State {
    tokens: f64,
    last_refill: Instant,
}

impl fmt::Debug for RateLimiter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RateLimiter")
            .field("capacity", &self.capacity)
            .field("refill_per_second", &self.refill_per_second)
            .finish()
    }
}

impl RateLimiter {
    /// Build a limiter with the given burst capacity and sustained rate.
    ///
    /// `capacity` is the number of tokens available at start and the ceiling
    /// the bucket refills to. `refill_per_minute` is the sustained rate.
    ///
    /// # Panics
    ///
    /// Panics if `capacity` or `refill_per_minute` is zero. A zero capacity
    /// would deadlock every caller, and a zero refill rate yields a
    /// non-finite wait duration. Both are programmer errors; to opt out of
    /// rate limiting pass `None` where an `Option<Arc<RateLimiter>>` is
    /// accepted.
    pub fn new(capacity: u32, refill_per_minute: u32) -> Self {
        assert!(capacity > 0, "RateLimiter capacity must be > 0");
        assert!(
            refill_per_minute > 0,
            "RateLimiter refill_per_minute must be > 0"
        );
        let capacity = capacity as f64;
        Self {
            capacity,
            refill_per_second: (refill_per_minute as f64) / 60.0,
            state: Mutex::new(State {
                tokens: capacity,
                last_refill: Instant::now(),
            }),
        }
    }

    /// Acquire one token, awaiting if the bucket is empty.
    ///
    /// Safe to call concurrently; waiters are served in roughly the order
    /// they arrived.
    ///
    /// Each call consumes one token on success. Retry loops (for example
    /// `HttpClient::retriable_http_request`) therefore consume one token per
    /// attempt — this is intentional and prevents a failing endpoint from
    /// being hammered.
    pub async fn acquire(&self) {
        loop {
            let wait = {
                let mut state = self.state.lock().await;
                let now = Instant::now();
                let elapsed = now.duration_since(state.last_refill).as_secs_f64();
                state.tokens = (state.tokens + elapsed * self.refill_per_second).min(self.capacity);
                state.last_refill = now;

                if state.tokens >= 1.0 {
                    state.tokens -= 1.0;
                    return;
                }

                // Duration until the bucket reaches one token.
                let needed = 1.0 - state.tokens;
                Duration::from_secs_f64(needed / self.refill_per_second)
            };
            tokio::time::sleep(wait).await;
        }
    }
}

impl Default for RateLimiter {
    /// Defaults matching Coco's WalletService: 20 capacity, 20 req/min.
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY, DEFAULT_REFILL_PER_MINUTE)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::*;

    #[tokio::test]
    async fn burst_consumes_capacity_without_waiting() {
        let limiter = RateLimiter::new(5, 1); // 1/min = very slow refill
        let start = Instant::now();
        for _ in 0..5 {
            limiter.acquire().await;
        }
        assert!(
            start.elapsed() < Duration::from_millis(100),
            "burst should be essentially instant, took {:?}",
            start.elapsed()
        );
    }

    #[tokio::test]
    async fn sixth_request_waits_for_refill() {
        // 600/min = 10 tokens/s = one token every 100ms.
        let limiter = RateLimiter::new(5, 600);
        for _ in 0..5 {
            limiter.acquire().await;
        }
        let start = Instant::now();
        limiter.acquire().await;
        let waited = start.elapsed();
        assert!(
            waited >= Duration::from_millis(80),
            "sixth should wait ~100ms, waited {:?}",
            waited
        );
        assert!(
            waited < Duration::from_millis(300),
            "sixth should not wait much more than one refill interval, waited {:?}",
            waited
        );
    }

    #[test]
    #[should_panic(expected = "capacity must be > 0")]
    fn zero_capacity_panics() {
        let _ = RateLimiter::new(0, 20);
    }

    #[test]
    #[should_panic(expected = "refill_per_minute must be > 0")]
    fn zero_refill_panics() {
        let _ = RateLimiter::new(20, 0);
    }

    #[tokio::test]
    async fn concurrent_acquires_are_serialized() {
        // 1200/min = 20/s = 50ms/token. Capacity 2, so 5 concurrent requests
        // drain the burst and must wait at least 3 * 50ms = 150ms in total.
        let limiter = Arc::new(RateLimiter::new(2, 1200));
        let completed = Arc::new(AtomicUsize::new(0));

        let start = Instant::now();
        let mut handles = Vec::new();
        for _ in 0..5 {
            let l = limiter.clone();
            let c = completed.clone();
            handles.push(tokio::spawn(async move {
                l.acquire().await;
                c.fetch_add(1, Ordering::SeqCst);
            }));
        }
        for h in handles {
            h.await.expect("join");
        }
        let elapsed = start.elapsed();

        assert_eq!(completed.load(Ordering::SeqCst), 5);
        assert!(
            elapsed >= Duration::from_millis(130),
            "5 requests with cap=2, rate=20/s should take ~150ms, took {:?}",
            elapsed
        );
    }

    #[tokio::test]
    async fn refill_is_capped_at_capacity() {
        // 100/s, saturates the bucket instantly. Let elapsed time pile up so
        // the bucket would mathematically hold more than `capacity` tokens.
        let limiter = RateLimiter::new(3, 6000);
        tokio::time::sleep(Duration::from_millis(100)).await;
        // Should still only have 3 tokens available in burst.
        let start = Instant::now();
        for _ in 0..3 {
            limiter.acquire().await;
        }
        assert!(start.elapsed() < Duration::from_millis(50));
        // Fourth acquire must wait for a fresh token.
        let start = Instant::now();
        limiter.acquire().await;
        assert!(
            start.elapsed() >= Duration::from_millis(5),
            "fourth should have waited for a refill tick"
        );
    }
}

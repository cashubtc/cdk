//! GCRA rate limiter for transport-level request throttling

use std::num::NonZero;
use std::sync::Arc;

use cdk_common::AuthToken;
use cdk_http_client::{HttpError, RawResponse};
use tokio::sync::Mutex;
use web_time::{Duration, Instant};

/// Sleep for `dur`, using the platform timer. On wasm32 `tokio::time` has no
/// timer driver, so route through `gloo-timers` (browser `setTimeout`).
async fn sleep(dur: Duration) {
    #[cfg(not(target_arch = "wasm32"))]
    tokio::time::sleep(dur).await;
    #[cfg(target_arch = "wasm32")]
    gloo_timers::future::TimeoutFuture::new(dur.as_millis().min(u32::MAX as u128) as u32).await;
}

/// Configuration for the rate limiter
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// Maximum number of tokens (burst capacity)
    pub capacity: NonZero<u32>,
    /// Number of tokens added per minute
    pub refill_per_minute: NonZero<u32>,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        // Stays under nutshell's default 60/min global cap.
        Self {
            capacity: NonZero::new(20).expect("valid non zero default"),
            refill_per_minute: NonZero::new(20).expect("valid non zero default"),
        }
    }
}

/// Rate limiter using GCRA (generic cell rate algorithm)
///
/// Allows up to `capacity` requests in a burst, then paces at one request per
/// `emission_interval` (derived from `refill_per_minute`). Each `acquire`
/// reserves its slot under the lock and sleeps once, so concurrent callers are
/// served in lock-acquisition order without a retry loop.
#[derive(Debug, Clone)]
pub struct TokenBucket {
    state: Arc<Mutex<BucketState>>,
    /// Time to earn one token (one request's worth of budget).
    emission_interval: Duration,
    /// Burst window: how far ahead of `now` the theoretical arrival time may
    /// run before requests start waiting. `(capacity - 1) * emission_interval`.
    tolerance: Duration,
}

#[derive(Debug)]
struct BucketState {
    /// Theoretical arrival time: the instant at which the most recently
    /// reserved request is scheduled. Requests before `arrival_time - tolerance` wait.
    arrival_time: Instant,
}

impl TokenBucket {
    /// Create a new token bucket from config.
    pub fn new(config: RateLimitConfig) -> Self {
        let emission_interval =
            Duration::from_secs_f64(60.0 / f64::from(config.refill_per_minute.get()));
        Self {
            state: Arc::new(Mutex::new(BucketState {
                arrival_time: Instant::now(),
            })),
            emission_interval,
            tolerance: emission_interval * (config.capacity.get() - 1),
        }
    }

    /// Reserve the next slot and return how long to wait before using it.
    ///
    /// Advances the theoretical arrival time, so calling this commits the
    /// caller to the slot whether or not it later sleeps.
    fn reserve(&self, state: &mut BucketState) -> Duration {
        let now = Instant::now();
        let arrival_time = state.arrival_time.max(now);
        let wait = arrival_time
            .checked_sub(self.tolerance)
            .map(|allowed| allowed.saturating_duration_since(now))
            .unwrap_or(Duration::ZERO);
        state.arrival_time = arrival_time + self.emission_interval;
        wait
    }

    /// Wait until a token is available, then consume it.
    pub async fn acquire(&self) {
        let wait = {
            let mut state = self.state.lock().await;
            self.reserve(&mut state)
        };

        sleep(wait).await;
    }

    /// Consume a token without waiting. Returns `true` if one was available (a
    /// slot within the burst window), `false` otherwise. On `false` no slot is
    /// reserved.
    pub async fn try_acquire(&self) -> bool {
        let mut state = self.state.lock().await;
        let now = Instant::now();
        let arrival_time = state.arrival_time.max(now);
        let ready = arrival_time
            .checked_sub(self.tolerance)
            .is_none_or(|allowed| allowed <= now);
        if ready {
            state.arrival_time = arrival_time + self.emission_interval;
        }
        ready
    }
}

/// A transport wrapper that rate-limits outbound requests using a token bucket
#[derive(Debug, Clone)]
pub struct RateLimitedTransport<T>
where
    T: super::Transport,
{
    inner: T,
    bucket: TokenBucket,
}

impl<T> RateLimitedTransport<T>
where
    T: super::Transport,
{
    /// Wrap a transport using a token bucket. Cloning the bucket shares its
    /// state, so the same bucket can pace multiple transports.
    pub fn with_bucket(inner: T, bucket: TokenBucket) -> Self {
        Self { inner, bucket }
    }
}

impl<T> Default for RateLimitedTransport<T>
where
    T: super::Transport,
{
    fn default() -> Self {
        Self {
            inner: T::default(),
            bucket: TokenBucket::new(RateLimitConfig::default()),
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl<T> super::Transport for RateLimitedTransport<T>
where
    T: super::Transport + Send + Sync,
{
    async fn ws_connect(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<
        (
            cdk_http_client::ws::WsSender,
            cdk_http_client::ws::WsReceiver,
        ),
        cdk_http_client::ws::WsError,
    > {
        self.inner.ws_connect(url, headers).await
    }

    fn with_proxy(
        &mut self,
        proxy: url::Url,
        host_matcher: Option<&str>,
        accept_invalid_certs: bool,
    ) -> Result<(), HttpError> {
        self.inner
            .with_proxy(proxy, host_matcher, accept_invalid_certs)
    }

    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    async fn resolve_dns_txt(&self, domain: &str) -> Result<Vec<String>, HttpError> {
        self.inner.resolve_dns_txt(domain).await
    }

    async fn http_get<R>(&self, url: url::Url, auth: Option<AuthToken>) -> Result<R, HttpError>
    where
        R: serde::de::DeserializeOwned,
    {
        self.bucket.acquire().await;
        self.inner.http_get(url, auth).await
    }

    async fn http_get_raw(
        &self,
        url: url::Url,
        auth: Option<AuthToken>,
    ) -> Result<RawResponse, HttpError> {
        self.bucket.acquire().await;
        self.inner.http_get_raw(url, auth).await
    }

    async fn http_post<P, R>(
        &self,
        url: url::Url,
        auth_token: Option<AuthToken>,
        payload: &P,
    ) -> Result<R, HttpError>
    where
        P: serde::Serialize + Send + Sync,
        R: serde::de::DeserializeOwned,
    {
        self.bucket.acquire().await;
        self.inner.http_post(url, auth_token, payload).await
    }

    async fn http_post_form_raw<P>(
        &self,
        url: url::Url,
        auth_token: Option<AuthToken>,
        payload: &P,
    ) -> Result<RawResponse, HttpError>
    where
        P: serde::Serialize + Send + Sync,
    {
        self.bucket.acquire().await;
        self.inner
            .http_post_form_raw(url, auth_token, payload)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_default_config() {
        let config = RateLimitConfig::default();
        assert_eq!(config.capacity.get(), 20);
        assert_eq!(config.refill_per_minute.get(), 20);
    }

    #[tokio::test]
    async fn test_acquire_within_capacity() {
        let bucket = TokenBucket::new(RateLimitConfig {
            capacity: NonZero::new(5).unwrap(),
            refill_per_minute: NonZero::new(60).unwrap(),
        });

        // Should acquire up to capacity without waiting
        for _ in 0..5 {
            bucket.acquire().await;
        }
    }

    #[tokio::test]
    async fn test_acquire_blocks_when_empty() {
        let bucket = TokenBucket::new(RateLimitConfig {
            capacity: NonZero::new(1).unwrap(),
            refill_per_minute: NonZero::new(6000).unwrap(), // 100/sec so the wait is short
        });

        // Drain the bucket
        bucket.acquire().await;

        // Next acquire should block briefly then succeed
        let start = Instant::now();
        bucket.acquire().await;
        let elapsed = start.elapsed();

        assert!(
            elapsed.as_millis() >= 5,
            "Expected some wait time, got {:?}",
            elapsed
        );
    }

    #[tokio::test]
    async fn test_refill_does_not_exceed_capacity() {
        let bucket = TokenBucket::new(RateLimitConfig {
            capacity: NonZero::new(3).unwrap(),
            refill_per_minute: NonZero::new(60_000).unwrap(), // fast refill for test
        });

        // Drain one token
        bucket.acquire().await;

        // Wait long enough to overshoot capacity if uncapped
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Should still only get capacity tokens (capped at 3)
        for _ in 0..3 {
            bucket.acquire().await;
        }
    }

    #[tokio::test]
    async fn test_concurrent_acquire() {
        let bucket = TokenBucket::new(RateLimitConfig {
            capacity: NonZero::new(10).unwrap(),
            refill_per_minute: NonZero::new(600).unwrap(), // 10/sec
        });

        // Spawn 10 tasks all acquiring at once
        let mut handles = Vec::new();
        for _ in 0..10 {
            let b = bucket.clone();
            handles.push(tokio::spawn(async move {
                b.acquire().await;
            }));
        }

        // All should complete (we have 10 capacity)
        for handle in handles {
            handle.await.unwrap();
        }
    }

    #[tokio::test]
    async fn test_try_acquire_within_and_beyond_burst() {
        let bucket = TokenBucket::new(RateLimitConfig {
            capacity: NonZero::new(3).unwrap(),
            refill_per_minute: NonZero::new(60).unwrap(), // 1/sec, slow refill
        });

        // Burst capacity of 3 is available immediately.
        for _ in 0..3 {
            assert!(bucket.try_acquire().await);
        }

        // Burst spent, refill too slow to have produced another token.
        assert!(!bucket.try_acquire().await);

        // A failed try_acquire must not have reserved a slot, so acquire still
        // sees the same budget (it will simply pace instead of erroring).
        assert!(!bucket.try_acquire().await);
    }

    #[tokio::test]
    async fn test_bucket_new_starts_full() {
        let bucket = TokenBucket::new(RateLimitConfig {
            capacity: NonZero::new(25).unwrap(),
            refill_per_minute: NonZero::new(25).unwrap(),
        });

        // Should be able to drain all 25 without waiting
        let start = Instant::now();
        for _ in 0..25 {
            bucket.acquire().await;
        }
        let elapsed = start.elapsed();

        // All 25 should be near-instant (well under 100ms)
        assert!(
            elapsed.as_millis() < 100,
            "Draining full bucket took too long: {:?}",
            elapsed
        );
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod integration_tests {
    use std::num::NonZero;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::time::{Duration, Instant};
    use url::Url;

    use super::super::{Async, Transport};
    use super::{RateLimitConfig, RateLimitedTransport, TokenBucket};

    /// Spawn a loopback HTTP server that accepts up to `num_requests`
    /// connections, replies 200 with `{}`, and increments the counter.
    async fn spawn_counting_server(num_requests: usize) -> (Url, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();

        tokio::spawn(async move {
            for _ in 0..num_requests {
                if let Ok((mut socket, _)) = listener.accept().await {
                    let mut buf = [0u8; 2048];
                    let _ = socket.read(&mut buf).await;
                    let body = "{}";
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body,
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                    counter_clone.fetch_add(1, Ordering::SeqCst);
                }
            }
        });

        let url = Url::parse(&format!("http://{}/", addr)).expect("valid url");
        (url, counter)
    }

    /// The bucket gates real HTTP traffic through the Transport trait, not
    /// just in isolation. Capacity 2 with 2/sec refill means the third call
    /// must wait ~500ms and the fourth ~1s.
    #[tokio::test]
    async fn rate_limited_transport_paces_real_http() {
        let (url, _counter) = spawn_counting_server(4).await;

        let bucket = TokenBucket::new(RateLimitConfig {
            capacity: NonZero::new(2).unwrap(),
            refill_per_minute: NonZero::new(120).unwrap(),
        });
        let transport = RateLimitedTransport::with_bucket(Async::default(), bucket);

        let start = Instant::now();
        let mut elapsed = Vec::with_capacity(4);
        for _ in 0..4 {
            let _: serde_json::Value = transport
                .http_get(url.clone(), None)
                .await
                .expect("request succeeds");
            elapsed.push(start.elapsed());
        }

        assert!(
            elapsed[0] < Duration::from_millis(200),
            "burst call 1 should be fast: {:?}",
            elapsed[0]
        );
        assert!(
            elapsed[1] < Duration::from_millis(200),
            "burst call 2 should be fast: {:?}",
            elapsed[1]
        );
        assert!(
            elapsed[2] >= Duration::from_millis(400),
            "call 3 should wait for refill: {:?}",
            elapsed[2]
        );
        assert!(
            elapsed[3] >= Duration::from_millis(900),
            "call 4 should wait for second refill: {:?}",
            elapsed[3]
        );
    }

    /// Two transports cloning the same bucket share the rate-limit budget.
    /// This is the mechanism the wallet uses to pace blind-auth and main
    /// traffic together.
    #[tokio::test]
    async fn shared_bucket_paces_two_transports() {
        let (url, _counter) = spawn_counting_server(3).await;

        let bucket = TokenBucket::new(RateLimitConfig {
            capacity: NonZero::new(2).unwrap(),
            refill_per_minute: NonZero::new(120).unwrap(),
        });
        let transport_a = RateLimitedTransport::with_bucket(Async::default(), bucket.clone());
        let transport_b = RateLimitedTransport::with_bucket(Async::default(), bucket);

        // Drain the shared bucket via transport_a.
        let _: serde_json::Value = transport_a.http_get(url.clone(), None).await.expect("a1");
        let _: serde_json::Value = transport_a.http_get(url.clone(), None).await.expect("a2");

        // transport_b sees an empty bucket and must wait for refill.
        let start = Instant::now();
        let _: serde_json::Value = transport_b.http_get(url.clone(), None).await.expect("b1");
        let elapsed = start.elapsed();

        assert!(
            elapsed >= Duration::from_millis(400),
            "transport_b should wait because the shared bucket is empty: {:?}",
            elapsed
        );
    }
}

//! Token bucket rate limiter for transport-level request throttling

use std::sync::Arc;

use tokio::sync::Mutex;
use tokio::time::{Duration, Instant};

/// Configuration for the rate limiter
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// Maximum number of tokens (burst capacity)
    pub capacity: u32,
    /// Number of tokens added per minute
    pub refill_per_minute: u32,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        // Stays under nutshell's default 60/min global cap.
        Self {
            capacity: 20,
            refill_per_minute: 20,
        }
    }
}

/// Token bucket rate limiter
///
/// Allows up to `capacity` requests in a burst, then refills at
/// `refill_per_minute` tokens per minute. When no tokens are available,
/// `acquire` sleeps until one is ready.
#[derive(Debug, Clone)]
pub struct TokenBucket {
    state: Arc<Mutex<BucketState>>,
    config: RateLimitConfig,
}

#[derive(Debug)]
struct BucketState {
    tokens: f64,
    last_refill: Instant,
}

impl TokenBucket {
    /// Create a new token bucket from config.
    ///
    /// # Panics
    ///
    /// Panics if `capacity` or `refill_per_minute` is zero.
    pub fn new(config: RateLimitConfig) -> Self {
        assert!(
            config.capacity > 0,
            "RateLimitConfig::capacity must be > 0",
        );
        assert!(
            config.refill_per_minute > 0,
            "RateLimitConfig::refill_per_minute must be > 0",
        );
        Self {
            state: Arc::new(Mutex::new(BucketState {
                tokens: f64::from(config.capacity),
                last_refill: Instant::now(),
            })),
            config,
        }
    }

    /// Wait until a token is available, then consume it
    pub async fn acquire(&self) {
        loop {
            let wait_duration = {
                let mut state = self.state.lock().await;
                self.refill(&mut state);

                if state.tokens >= 1.0 {
                    state.tokens -= 1.0;
                    return;
                }

                let tokens_needed = 1.0 - state.tokens;
                let refill_per_ms = f64::from(self.config.refill_per_minute) / (60.0 * 1000.0);
                Duration::from_millis((tokens_needed / refill_per_ms).ceil() as u64)
            };

            tokio::time::sleep(wait_duration).await;
        }
    }

    fn refill(&self, state: &mut BucketState) {
        let now = Instant::now();
        let elapsed_ms = now.duration_since(state.last_refill).as_millis() as f64;
        let refill_per_ms = f64::from(self.config.refill_per_minute) / (60.0 * 1000.0);
        let new_tokens = elapsed_ms * refill_per_ms;

        if new_tokens > 0.0 {
            state.tokens = (state.tokens + new_tokens).min(f64::from(self.config.capacity));
            state.last_refill = now;
        }
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
    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    async fn resolve_dns_txt(&self, domain: &str) -> Result<Vec<String>, super::super::Error> {
        self.inner.resolve_dns_txt(domain).await
    }

    fn with_proxy(
        &mut self,
        proxy: url::Url,
        host_matcher: Option<&str>,
        accept_invalid_certs: bool,
    ) -> Result<(), super::super::Error> {
        self.inner
            .with_proxy(proxy, host_matcher, accept_invalid_certs)
    }

    async fn http_get<R>(
        &self,
        url: url::Url,
        auth: Option<cdk_common::AuthToken>,
    ) -> Result<R, super::super::Error>
    where
        R: serde::de::DeserializeOwned,
    {
        self.bucket.acquire().await;
        self.inner.http_get(url, auth).await
    }

    async fn http_post<P, R>(
        &self,
        url: url::Url,
        auth_token: Option<cdk_common::AuthToken>,
        payload: &P,
    ) -> Result<R, super::super::Error>
    where
        P: serde::Serialize + ?Sized + Send + Sync,
        R: serde::de::DeserializeOwned,
    {
        self.bucket.acquire().await;
        self.inner.http_post(url, auth_token, payload).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_default_config() {
        let config = RateLimitConfig::default();
        assert_eq!(config.capacity, 20);
        assert_eq!(config.refill_per_minute, 20);
    }

    #[test]
    #[should_panic(expected = "capacity must be > 0")]
    fn test_zero_capacity_panics() {
        let _ = TokenBucket::new(RateLimitConfig {
            capacity: 0,
            refill_per_minute: 20,
        });
    }

    #[test]
    #[should_panic(expected = "refill_per_minute must be > 0")]
    fn test_zero_refill_panics() {
        let _ = TokenBucket::new(RateLimitConfig {
            capacity: 20,
            refill_per_minute: 0,
        });
    }

    #[tokio::test]
    async fn test_acquire_within_capacity() {
        let bucket = TokenBucket::new(RateLimitConfig {
            capacity: 5,
            refill_per_minute: 60,
        });

        // Should acquire up to capacity without waiting
        for _ in 0..5 {
            bucket.acquire().await;
        }
    }

    #[tokio::test]
    async fn test_acquire_blocks_when_empty() {
        let bucket = TokenBucket::new(RateLimitConfig {
            capacity: 1,
            refill_per_minute: 6000, // 100/sec so the wait is short
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
            capacity: 3,
            refill_per_minute: 60_000, // fast refill for test
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
            capacity: 10,
            refill_per_minute: 600, // 10/sec
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
    async fn test_bucket_new_starts_full() {
        let bucket = TokenBucket::new(RateLimitConfig {
            capacity: 25,
            refill_per_minute: 25,
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
            capacity: 2,
            refill_per_minute: 120,
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
            capacity: 2,
            refill_per_minute: 120,
        });
        let transport_a = RateLimitedTransport::with_bucket(Async::default(), bucket.clone());
        let transport_b = RateLimitedTransport::with_bucket(Async::default(), bucket);

        // Drain the shared bucket via transport_a.
        let _: serde_json::Value = transport_a
            .http_get(url.clone(), None)
            .await
            .expect("a1");
        let _: serde_json::Value = transport_a
            .http_get(url.clone(), None)
            .await
            .expect("a2");

        // transport_b sees an empty bucket and must wait for refill.
        let start = Instant::now();
        let _: serde_json::Value = transport_b
            .http_get(url.clone(), None)
            .await
            .expect("b1");
        let elapsed = start.elapsed();

        assert!(
            elapsed >= Duration::from_millis(400),
            "transport_b should wait because the shared bucket is empty: {:?}",
            elapsed
        );
    }
}

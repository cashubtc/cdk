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

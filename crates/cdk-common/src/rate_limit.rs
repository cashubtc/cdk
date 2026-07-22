//! Client-side request rate limiting for the wallet.
//!
//! The wallet paces its outbound HTTP requests to a mint so it stays under the
//! mint's server-side request cap without the caller having to think about it.
//! Pacing uses the Generic Cell Rate Algorithm (GCRA): the bucket tracks a
//! single theoretical arrival time (TAT) rather than a refilling token count.
//!
//! The budget is persisted per mint host in the wallet key-value store, so a
//! wallet that is built, used, and dropped hands its remaining budget to the
//! next wallet built for the same mint instead of starting full and bursting
//! again.

use std::future::Future;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cdk_http_client::Transport;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::sync::{watch, OnceCell};
use url::Url;
use web_time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::database::{self, WalletDatabase, KVSTORE_NAMESPACE_KEY_MAX_LEN};
use crate::mint_url::MintUrl;
use crate::{AuthToken, HttpError, RawResponse};

/// Namespace under which per-host rate-limit budgets are stored.
const KV_NAMESPACE: &str = "rate_limiter";

/// Configuration for a [`TokenBucket`].
///
/// Both fields are `NonZeroU32`, so a zero or otherwise invalid configuration
/// cannot be constructed; there is no separate runtime validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitConfig {
    /// Maximum burst: how many requests may go out back-to-back before pacing
    /// kicks in.
    pub capacity: NonZeroU32,
    /// Sustained rate: how many requests are earned back per minute.
    pub refill_per_minute: NonZeroU32,
}

impl RateLimitConfig {
    /// Create a new configuration.
    pub fn new(capacity: NonZeroU32, refill_per_minute: NonZeroU32) -> Self {
        Self {
            capacity,
            refill_per_minute,
        }
    }

    /// Create a configuration from plain integers, returning `None` if either is
    /// zero. A convenience over building the `NonZeroU32`s by hand.
    pub fn try_new(capacity: u32, refill_per_minute: u32) -> Option<Self> {
        Some(Self::new(
            NonZeroU32::new(capacity)?,
            NonZeroU32::new(refill_per_minute)?,
        ))
    }
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        // Chosen to stay under a mint's per-minute cap (nutshell defaults to 60).
        // Over any 60s window a GCRA bucket admits at most `capacity + refill`
        // requests: one full burst plus the steady trickle. So the sum, not the
        // sustained rate alone, is what must stay under the cap. 10 + 45 = 55
        // keeps a margin under 60 while sustaining ~45 requests/minute, which is
        // far less punishing on bulk flows (restore, batch quote checks) than a
        // lower sustained rate would be. The `expect`s are on compile-time
        // constant literals that are trivially non-zero.
        Self {
            capacity: NonZeroU32::new(10).expect("10 is non-zero"),
            refill_per_minute: NonZeroU32::new(45).expect("45 is non-zero"),
        }
    }
}

/// The persistence backend for a bucket's budget: a single blob loaded once and
/// written back later. Kept as a narrow trait so the writer is decoupled from
/// `WalletDatabase` and can be exercised with a mock in tests.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
trait BudgetStore: std::fmt::Debug + Send + Sync {
    /// Read the persisted budget. `None` when absent or on error (logged here).
    async fn load(&self) -> Option<Vec<u8>>;
    /// Write the budget. Returns whether it was persisted.
    async fn store(&self, value: &[u8]) -> bool;
}

/// [`BudgetStore`] backed by the wallet key-value store, keyed by mint host.
#[derive(Debug)]
struct KvBudgetStore {
    db: Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
    key: String,
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl BudgetStore for KvBudgetStore {
    async fn load(&self) -> Option<Vec<u8>> {
        match self.db.kv_read(KV_NAMESPACE, "", &self.key).await {
            Ok(value) => value,
            Err(err) => {
                tracing::warn!("rate limiter failed to load persisted budget: {err}");
                None
            }
        }
    }

    async fn store(&self, value: &[u8]) -> bool {
        match self.db.kv_write(KV_NAMESPACE, "", &self.key, value).await {
            Ok(()) => true,
            Err(err) => {
                tracing::warn!("rate limiter failed to persist budget: {err}");
                false
            }
        }
    }
}

/// Mutable GCRA state.
#[derive(Debug)]
struct BucketState {
    /// Theoretical arrival time: the moment the most recently reserved request
    /// is scheduled for.
    arrival_time: Instant,
}

/// Handle to the single background writer for a persisted bucket.
///
/// `desired` publishes the latest budget (wall-clock TAT in millis) to the
/// writer; a `watch` keeps only the newest value, so bursts of updates coalesce
/// into one write. `progress` reports the latest value the writer has *attempted*
/// to store (regardless of success), which [`TokenBucket::flush`] awaits.
#[derive(Debug)]
struct Writer {
    desired: watch::Sender<u64>,
    progress: watch::Receiver<u64>,
}

#[derive(Debug)]
struct TokenBucketInner {
    /// Time to earn one request's worth of budget.
    emission_interval: Duration,
    /// Burst window: how far the arrival time may run ahead of now before
    /// callers must wait.
    tolerance: Duration,
    state: Mutex<BucketState>,
    persistence: Option<Arc<dyn BudgetStore>>,
    /// Loads the budget once and starts the single writer, both on first use.
    started: OnceCell<Option<Writer>>,
}

/// A GCRA rate limiter.
///
/// Cloning shares the underlying state through an `Arc`, so cloned buckets draw
/// down one shared budget. This is how the wallet's main client and its
/// blind-auth client are kept under a single cap.
#[derive(Debug, Clone)]
pub struct TokenBucket {
    inner: Arc<TokenBucketInner>,
}

impl TokenBucket {
    /// Create a bucket with no persistence. It starts full.
    pub fn new(config: RateLimitConfig) -> Self {
        Self::build(config, None)
    }

    /// Create a bucket that persists its budget for `mint_url` in `store`.
    ///
    /// If a host cannot be derived from `mint_url`, the bucket still paces
    /// requests in memory but persists nothing.
    pub fn for_mint(
        config: RateLimitConfig,
        mint_url: &MintUrl,
        db: Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
    ) -> Self {
        let persistence = kv_key_for(mint_url)
            .map(|key| Arc::new(KvBudgetStore { db, key }) as Arc<dyn BudgetStore>);
        Self::build(config, persistence)
    }

    /// Create a bucket persisting through a custom [`BudgetStore`] (test seam).
    #[cfg(test)]
    fn with_store(config: RateLimitConfig, store: Arc<dyn BudgetStore>) -> Self {
        Self::build(config, Some(store))
    }

    fn build(config: RateLimitConfig, persistence: Option<Arc<dyn BudgetStore>>) -> Self {
        let emission_interval = Duration::from_secs(60) / config.refill_per_minute.get();
        let tolerance = emission_interval * (config.capacity.get() - 1);

        Self {
            inner: Arc::new(TokenBucketInner {
                emission_interval,
                tolerance,
                state: Mutex::new(BucketState {
                    arrival_time: Instant::now(),
                }),
                persistence,
                started: OnceCell::new(),
            }),
        }
    }

    /// Wait until a slot is available, then run `action` and return its output.
    ///
    /// The slot is reserved (and handed to the background writer) before `action`
    /// runs, so the reservation stands whether or not the action succeeds.
    pub async fn acquire<F, T>(&self, action: F) -> T
    where
        F: Future<Output = T>,
    {
        let wait = self.reserve_slot().await;
        if !wait.is_zero() {
            sleep(wait).await;
        }
        action.await
    }

    /// Load the budget on first use, reserve the next slot, hand the new budget
    /// to the writer, and return how long the caller must wait before using the
    /// slot. Persistence never blocks here: `publish` only updates the cache.
    async fn reserve_slot(&self) -> Duration {
        self.ensure_started().await;
        let (wait, millis) = self.advance();
        self.publish(millis);
        wait
    }

    /// Consume a slot without waiting.
    ///
    /// Returns `true` and consumes a slot if one is available within the burst
    /// window; returns `false` and reserves nothing otherwise.
    pub async fn try_acquire(&self) -> bool {
        self.ensure_started().await;
        let reserved = {
            let mut state = lock(&self.inner.state);
            let base = state.arrival_time.max(Instant::now());
            let ahead = base.saturating_duration_since(Instant::now());
            if ahead > self.inner.tolerance {
                None
            } else {
                state.arrival_time = base + self.inner.emission_interval;
                Some(tat_to_unix_millis(state.arrival_time))
            }
        };
        match reserved {
            Some(millis) => {
                self.publish(millis);
                true
            }
            None => false,
        }
    }

    /// Advance the theoretical arrival time by one slot under the lock. Returns
    /// the wait before the slot may be used and the new budget (wall-clock TAT
    /// in millis) to publish, so the caller need not relock the state.
    fn advance(&self) -> (Duration, u64) {
        let mut state = lock(&self.inner.state);
        let now = Instant::now();
        let base = state.arrival_time.max(now);
        let ahead = base.saturating_duration_since(now);
        let wait = ahead.saturating_sub(self.inner.tolerance);
        state.arrival_time = base + self.inner.emission_interval;
        (wait, tat_to_unix_millis(state.arrival_time))
    }

    /// On first use, load the persisted budget once and start the single writer.
    ///
    /// The load seeds the in-memory cache; the writer then drains that cache to
    /// the store in the background. Runs exactly once via the `OnceCell`.
    async fn ensure_started(&self) {
        self.inner
            .started
            .get_or_init(|| async {
                let store = self.inner.persistence.clone()?;

                // Read the first: seed the cache from the persisted value.
                let loaded = match store.load().await {
                    Some(bytes) => <[u8; 8]>::try_from(bytes.as_slice())
                        .ok()
                        .map(u64::from_be_bytes),
                    None => None,
                };
                if let Some(stored) = loaded {
                    let tat_wall = UNIX_EPOCH + Duration::from_millis(stored);
                    // Clamp to the burst window so a corrupt or far-future stored
                    // value cannot wedge the wallet. The side effect is that a
                    // restart forgives any backpressure beyond one burst:
                    // inherited debt is never more than `tolerance`.
                    let debt = tat_wall
                        .duration_since(SystemTime::now())
                        .unwrap_or_default()
                        .min(self.inner.tolerance);
                    lock(&self.inner.state).arrival_time = Instant::now() + debt;
                }

                // Start the one writer. It owns only the store and channels (never
                // `Arc<Inner>`), so dropping the bucket closes `desired`, flushes
                // the final value, and lets the task exit.
                let seed = loaded.unwrap_or(0);
                let (desired_tx, desired_rx) = watch::channel(seed);
                let (progress_tx, progress_rx) = watch::channel(seed);
                crate::task::spawn(run_writer(store, desired_rx, progress_tx));
                Some(Writer {
                    desired: desired_tx,
                    progress: progress_rx,
                })
            })
            .await;
    }

    /// Hand `millis` (the latest budget) to the writer. Non-blocking: it only
    /// updates the cache. Intermediate values coalesce, so a slow store never
    /// slows requests and only the newest value is ever written.
    fn publish(&self, millis: u64) {
        if let Some(Some(writer)) = self.inner.started.get() {
            let _ = writer.desired.send(millis);
        }
    }

    /// Wait until the writer has attempted to persist at least the current
    /// budget, or `FLUSH_TIMEOUT` elapses.
    ///
    /// A durability barrier for graceful shutdown (and deterministic tests): the
    /// hot path never waits for the store, but a caller can. Completion is based
    /// on the writer having *attempted* the value, so a failing store still lets
    /// `flush` return; the timeout is a backstop for a store call that never
    /// completes. Returns early if the bucket has no persistence.
    pub async fn flush(&self) {
        self.ensure_started().await;
        let Some(Some(writer)) = self.inner.started.get() else {
            return;
        };
        let target = {
            let state = lock(&self.inner.state);
            tat_to_unix_millis(state.arrival_time)
        };
        // Only nudge the writer if `target` is newer than what's already cached;
        // otherwise the last reservation already published it, so avoid forcing a
        // redundant write.
        writer.desired.send_if_modified(|current| {
            if *current < target {
                *current = target;
                true
            } else {
                false
            }
        });
        let mut progress = writer.progress.clone();
        let wait = async {
            while *progress.borrow_and_update() < target {
                if progress.changed().await.is_err() {
                    break;
                }
            }
        };
        let _ = with_timeout(FLUSH_TIMEOUT, wait).await;
    }
}

/// The single writer task for a persisted bucket: persist the latest cached
/// value whenever it changes, coalescing intermediate updates. Persistence is
/// best effort (`store` logs its own failures); `progress` reports the latest
/// value attempted so [`TokenBucket::flush`] can make progress even when a write
/// fails.
///
/// Lifecycle: this task is spawned detached (the `JoinHandle` is dropped) and is
/// never force-killed. It ends itself when the last `desired` sender drops, i.e.
/// once every `TokenBucket` handle (and so `Arc<TokenBucketInner>`) has been
/// dropped: `changed()` then returns `Err`, the loop exits, a final value is
/// written, and the task returns. It owns only `store` and the channels (never
/// `Arc<TokenBucketInner>`), so it never keeps the bucket alive. One caveat:
/// closure is only observed at the `changed().await` point, so if the task is
/// parked in `store.store(...).await` on a hung store when the bucket drops, it
/// cannot finish until that call returns (or the runtime is torn down at process
/// exit); it still does not leak the bucket.
async fn run_writer(
    store: Arc<dyn BudgetStore>,
    mut desired: watch::Receiver<u64>,
    progress: watch::Sender<u64>,
) {
    while desired.changed().await.is_ok() {
        let millis = *desired.borrow_and_update();
        store.store(&millis.to_be_bytes()).await;
        let _ = progress.send(millis);
    }
    // All `desired` senders dropped (bucket gone): flush the final value once.
    let millis = *desired.borrow();
    store.store(&millis.to_be_bytes()).await;
    let _ = progress.send(millis);
}

/// Upper bound on how long [`TokenBucket::flush`] waits before giving up, so a
/// store call that never returns cannot hang graceful shutdown.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(5);

/// Race `fut` against a timeout using the cross-platform [`sleep`]. Returns
/// `None` if the timeout elapses first. Used instead of `tokio::time::timeout`
/// because that has no driver under wasm.
async fn with_timeout<F: Future>(duration: Duration, fut: F) -> Option<F::Output> {
    use futures::future::{select, Either};

    let fut = std::pin::pin!(fut);
    let timeout = std::pin::pin!(sleep(duration));
    match select(fut, timeout).await {
        Either::Left((output, _)) => Some(output),
        Either::Right(((), _)) => None,
    }
}

/// Recover a poisoned lock rather than panicking; the guarded state is plain
/// data and stays valid across a poisoning.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Translate a monotonic arrival time into a persistable wall-clock timestamp
/// (milliseconds since the Unix epoch).
fn tat_to_unix_millis(arrival_time: Instant) -> u64 {
    let ahead = arrival_time
        .checked_duration_since(Instant::now())
        .unwrap_or_default();
    (SystemTime::now() + ahead)
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Derive a KV key from a mint URL's host (and port), sanitized to the KV
/// alphabet. Returns `None` when the URL has no host.
///
/// The key is host (plus explicit port); scheme and default ports are ignored,
/// so `http`/`https` on the same host, or `:443` vs an implicit default, share
/// one budget. That is intended: one mint host is one budget.
///
/// Mapping disallowed characters to `_` (and truncating at the max length) can
/// make two distinct hosts collide onto one key. That is safe here: a collision
/// only makes two mints share (and so more conservatively enforce) one budget,
/// it never leaks budget between them.
fn kv_key_for(mint_url: &MintUrl) -> Option<String> {
    let parsed = Url::parse(&mint_url.to_string()).ok()?;
    let host = parsed.host_str()?;
    let authority = match parsed.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    };
    let sanitized: String = authority
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(KVSTORE_NAMESPACE_KEY_MAX_LEN)
        .collect();
    if sanitized.is_empty() {
        None
    } else {
        Some(sanitized)
    }
}

#[cfg(not(target_arch = "wasm32"))]
async fn sleep(duration: Duration) {
    tokio::time::sleep(duration).await;
}

#[cfg(target_arch = "wasm32")]
async fn sleep(duration: Duration) {
    // `tokio::time` has no timer driver under wasm; fall back to the browser's
    // `setTimeout` via gloo.
    gloo_timers::future::TimeoutFuture::new(duration.as_millis() as u32).await;
}

/// A [`Transport`] decorator that paces HTTP requests through a [`TokenBucket`].
///
/// Only the HTTP request methods are throttled; `ws_connect`, `with_proxy`, and
/// `resolve_dns_txt` pass straight through to the inner transport.
#[derive(Debug, Clone)]
pub struct RateLimitedTransport<T> {
    inner: T,
    bucket: TokenBucket,
}

impl<T> RateLimitedTransport<T> {
    /// Wrap `inner` with a (possibly shared) `bucket`.
    pub fn with_bucket(inner: T, bucket: TokenBucket) -> Self {
        Self { inner, bucket }
    }
}

impl<T: Default> Default for RateLimitedTransport<T> {
    fn default() -> Self {
        Self::with_bucket(T::default(), TokenBucket::new(RateLimitConfig::default()))
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl<T: Transport> Transport for RateLimitedTransport<T> {
    fn with_proxy(
        &mut self,
        proxy: Url,
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

    async fn http_get<R>(&self, url: Url, auth: Option<AuthToken>) -> Result<R, HttpError>
    where
        R: DeserializeOwned,
    {
        self.bucket.acquire(self.inner.http_get(url, auth)).await
    }

    async fn http_get_raw(
        &self,
        url: Url,
        auth: Option<AuthToken>,
    ) -> Result<RawResponse, HttpError> {
        self.bucket
            .acquire(self.inner.http_get_raw(url, auth))
            .await
    }

    async fn http_post<P, R>(
        &self,
        url: Url,
        auth_token: Option<AuthToken>,
        payload: &P,
    ) -> Result<R, HttpError>
    where
        P: Serialize + Send + Sync,
        R: DeserializeOwned,
    {
        self.bucket
            .acquire(self.inner.http_post(url, auth_token, payload))
            .await
    }

    async fn http_post_form_raw<P>(
        &self,
        url: Url,
        auth_token: Option<AuthToken>,
        payload: &P,
    ) -> Result<RawResponse, HttpError>
    where
        P: Serialize + Send + Sync,
    {
        self.bucket
            .acquire(self.inner.http_post_form_raw(url, auth_token, payload))
            .await
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::str::FromStr;
    use std::time::Instant as StdInstant;

    use super::*;

    fn config(capacity: u32, refill_per_minute: u32) -> RateLimitConfig {
        RateLimitConfig::new(
            NonZeroU32::new(capacity).unwrap_or(NonZeroU32::MIN),
            NonZeroU32::new(refill_per_minute).unwrap_or(NonZeroU32::MIN),
        )
    }

    #[test]
    fn default_config_values() {
        let cfg = RateLimitConfig::default();
        assert_eq!(cfg.capacity.get(), 10);
        assert_eq!(cfg.refill_per_minute.get(), 45);
        // The worst-case 60s window (capacity + refill) must stay under a mint's
        // typical 60/min cap.
        assert!(cfg.capacity.get() + cfg.refill_per_minute.get() < 60);
    }

    #[test]
    fn try_new_rejects_zero() {
        assert!(RateLimitConfig::try_new(0, 45).is_none());
        assert!(RateLimitConfig::try_new(10, 0).is_none());
        let cfg = RateLimitConfig::try_new(10, 45).expect("non-zero");
        assert_eq!(cfg.capacity.get(), 10);
        assert_eq!(cfg.refill_per_minute.get(), 45);
    }

    /// A minimal [`BudgetStore`] that can be made to fail every write.
    #[derive(Debug, Default)]
    struct StubStore {
        fail: bool,
    }

    #[async_trait]
    impl BudgetStore for StubStore {
        async fn load(&self) -> Option<Vec<u8>> {
            None
        }
        async fn store(&self, _value: &[u8]) -> bool {
            !self.fail
        }
    }

    /// A [`BudgetStore`] whose writes block on a semaphore the test controls, so
    /// coalescing can be asserted deterministically regardless of scheduling.
    #[derive(Debug)]
    struct GatedStore {
        loads: Arc<std::sync::atomic::AtomicUsize>,
        writes: Arc<Mutex<Vec<u64>>>,
        gate: Arc<tokio::sync::Semaphore>,
    }

    #[async_trait]
    impl BudgetStore for GatedStore {
        async fn load(&self) -> Option<Vec<u8>> {
            self.loads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            None
        }
        async fn store(&self, value: &[u8]) -> bool {
            // Block until the test releases writes.
            let _permit = self.gate.acquire().await.expect("gate not closed");
            if let Ok(raw) = <[u8; 8]>::try_from(value) {
                lock(&self.writes).push(u64::from_be_bytes(raw));
            }
            true
        }
    }

    #[tokio::test]
    async fn writer_coalesces_to_the_latest_value() {
        let store = Arc::new(GatedStore {
            loads: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            writes: Arc::new(Mutex::new(Vec::new())),
            gate: Arc::new(tokio::sync::Semaphore::new(0)),
        });
        // Huge burst so nothing paces; we only care about persistence here.
        let bucket = TokenBucket::with_store(config(1000, 60000), store.clone());

        // 50 reservations publish 50 values while the writer is gated on its
        // first store(); the watch coalesces them to the newest.
        for _ in 0..50 {
            bucket.acquire(async {}).await;
        }
        // Release writes, then flush so we know the writer has run.
        store.gate.add_permits(10);
        bucket.flush().await;

        let writes = lock(&store.writes).clone();
        assert!(!writes.is_empty(), "the latest value must be persisted");
        // At most one in-flight write plus the coalesced latest: far below 50.
        assert!(
            writes.len() <= 2,
            "writes should coalesce, got {}",
            writes.len()
        );
        // Only newer values are written.
        assert!(writes.windows(2).all(|w| w[0] <= w[1]));
        // load() ran exactly once across all those acquires.
        assert_eq!(store.loads.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn failing_store_never_blocks_acquire() {
        let store = Arc::new(StubStore { fail: true });
        let bucket = TokenBucket::with_store(config(5, 300), store);

        // Even though every write fails, the burst is served instantly: the
        // request path never waits on the store.
        let start = StdInstant::now();
        for _ in 0..5 {
            bucket.acquire(async {}).await;
        }
        assert!(start.elapsed() < Duration::from_millis(100));
    }

    #[tokio::test]
    async fn flush_returns_on_failing_store() {
        let store = Arc::new(StubStore { fail: true });
        let bucket = TokenBucket::with_store(config(5, 300), store);
        bucket.acquire(async {}).await;

        // flush must complete even though every write fails: the writer reports
        // progress on attempt, not only on success.
        tokio::time::timeout(Duration::from_secs(2), bucket.flush())
            .await
            .expect("flush must not hang on a failing store");
    }

    #[test]
    fn kv_key_is_sanitized() {
        let url = MintUrl::from_str("https://mint.example.com:3338").unwrap();
        let key = kv_key_for(&url).expect("host present");
        assert!(!key.contains('.'));
        assert!(!key.contains(':'));
        assert!(key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));
    }

    // Timing tests use a 200ms emission interval (refill 300/min) so the pace
    // signal sits well clear of scheduler noise: an unthrottled burst finishes
    // in well under 100ms, a paced request waits ~200ms.
    #[tokio::test]
    async fn fresh_bucket_starts_full() {
        let bucket = TokenBucket::new(config(5, 300));
        let start = StdInstant::now();
        for _ in 0..5 {
            bucket.acquire(async {}).await;
        }
        assert!(start.elapsed() < Duration::from_millis(100));
    }

    #[tokio::test]
    async fn acquiring_past_capacity_blocks() {
        // capacity 3, emission ~200ms, tolerance ~400ms.
        let bucket = TokenBucket::new(config(3, 300));
        for _ in 0..3 {
            bucket.acquire(async {}).await;
        }
        let start = StdInstant::now();
        bucket.acquire(async {}).await;
        assert!(start.elapsed() >= Duration::from_millis(150));
    }

    #[tokio::test]
    async fn try_acquire_respects_burst() {
        // capacity 2 => two immediate successes, then failure without reserving.
        let bucket = TokenBucket::new(config(2, 600));
        assert!(bucket.try_acquire().await);
        assert!(bucket.try_acquire().await);
        assert!(!bucket.try_acquire().await);
        // A failed try_acquire reserves nothing, so it stays false.
        assert!(!bucket.try_acquire().await);
    }

    #[tokio::test]
    async fn clones_share_one_budget() {
        let bucket = TokenBucket::new(config(2, 600));
        let clone = bucket.clone();
        assert!(bucket.try_acquire().await);
        assert!(clone.try_acquire().await);
        // Both slots are gone across the two handles.
        assert!(!bucket.try_acquire().await);
        assert!(!clone.try_acquire().await);
    }

    #[tokio::test]
    async fn concurrent_acquires_all_complete() {
        let bucket = TokenBucket::new(config(4, 6000));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let bucket = bucket.clone();
            handles.push(tokio::spawn(async move { bucket.acquire(async {}).await }));
        }
        for handle in handles {
            handle.await.unwrap();
        }
    }

    /// A transport that counts HTTP calls and never touches the network, so we
    /// can observe the decorator's pacing and pass-through without real I/O.
    #[derive(Debug, Clone, Default)]
    struct CountingTransport {
        http_calls: Arc<std::sync::atomic::AtomicUsize>,
        proxied: Arc<std::sync::atomic::AtomicBool>,
    }

    impl CountingTransport {
        fn http_calls(&self) -> usize {
            self.http_calls.load(std::sync::atomic::Ordering::SeqCst)
        }
        fn bump(&self) {
            self.http_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl Transport for CountingTransport {
        fn with_proxy(
            &mut self,
            _proxy: Url,
            _host_matcher: Option<&str>,
            _accept_invalid_certs: bool,
        ) -> Result<(), HttpError> {
            self.proxied
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }

        async fn http_get<R>(&self, _url: Url, _auth: Option<AuthToken>) -> Result<R, HttpError>
        where
            R: DeserializeOwned,
        {
            self.bump();
            Err(HttpError::Other("mock".to_string()))
        }

        async fn http_get_raw(
            &self,
            _url: Url,
            _auth: Option<AuthToken>,
        ) -> Result<RawResponse, HttpError> {
            self.bump();
            Err(HttpError::Other("mock".to_string()))
        }

        async fn http_post<P, R>(
            &self,
            _url: Url,
            _auth: Option<AuthToken>,
            _payload: &P,
        ) -> Result<R, HttpError>
        where
            P: Serialize + Send + Sync,
            R: DeserializeOwned,
        {
            self.bump();
            Err(HttpError::Other("mock".to_string()))
        }

        async fn http_post_form_raw<P>(
            &self,
            _url: Url,
            _auth: Option<AuthToken>,
            _payload: &P,
        ) -> Result<RawResponse, HttpError>
        where
            P: Serialize + Send + Sync,
        {
            self.bump();
            Err(HttpError::Other("mock".to_string()))
        }

        #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
        async fn resolve_dns_txt(&self, _domain: &str) -> Result<Vec<String>, HttpError> {
            Ok(Vec::new())
        }
    }

    fn url() -> Url {
        Url::parse("http://localhost/").expect("valid url")
    }

    #[tokio::test]
    async fn transport_paces_http_and_delegates() {
        // capacity 2, emission ~200ms: two calls burst, the third is paced.
        let inner = CountingTransport::default();
        let counter = inner.clone();
        let transport = RateLimitedTransport::with_bucket(inner, TokenBucket::new(config(2, 300)));

        let start = StdInstant::now();
        let _ = transport.http_get_raw(url(), None).await;
        let _ = transport.http_get_raw(url(), None).await;
        assert!(
            start.elapsed() < Duration::from_millis(100),
            "burst should not pace"
        );

        let start = StdInstant::now();
        let _ = transport.http_get_raw(url(), None).await;
        assert!(
            start.elapsed() >= Duration::from_millis(150),
            "third call should be paced"
        );

        // Every call reached the inner transport.
        assert_eq!(counter.http_calls(), 3);
    }

    #[tokio::test]
    async fn transport_passes_proxy_through_unthrottled() {
        let inner = CountingTransport::default();
        let flag = inner.proxied.clone();
        let counter = inner.clone();
        let mut transport =
            RateLimitedTransport::with_bucket(inner, TokenBucket::new(config(1, 300)));

        transport.with_proxy(url(), None, false).expect("proxy set");
        // with_proxy reached the inner transport and consumed no rate-limit slot.
        assert!(flag.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(counter.http_calls(), 0);
    }

    #[tokio::test]
    async fn transports_sharing_a_bucket_share_the_budget() {
        let bucket = TokenBucket::new(config(1, 300));
        let a = RateLimitedTransport::with_bucket(CountingTransport::default(), bucket.clone());
        let b = RateLimitedTransport::with_bucket(CountingTransport::default(), bucket.clone());

        // Drain the single burst slot through transport A.
        let _ = a.http_get_raw(url(), None).await;

        // Transport B, sharing the same bucket, must now wait.
        let start = StdInstant::now();
        let _ = b.http_get_raw(url(), None).await;
        assert!(
            start.elapsed() >= Duration::from_millis(150),
            "shared bucket should force B to pace"
        );
    }
}

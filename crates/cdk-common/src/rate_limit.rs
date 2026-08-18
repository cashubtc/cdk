//! Client-side request rate limiting for the wallet (GCRA token bucket plus an
//! HTTP transport decorator).
//!
//! The wallet paces its outbound HTTP requests so it stays under a server's
//! request cap without the caller having to think about it. Pacing uses the
//! Generic Cell Rate Algorithm (GCRA): the bucket tracks a single theoretical
//! arrival time (TAT) rather than a refilling token count.
//!
//! A cap is a property of the host being called, not of the wallet doing the
//! calling, so budgets are keyed by the *destination* host of each request (see
//! [`RateLimiterManager`](crate::rate_limit::RateLimiterManager)). A wallet's
//! transport also carries requests that have
//! nothing to do with its mint (LNURL services, OIDC providers), and those must
//! not draw down the mint's budget.
//!
//! The budget is persisted per host in the wallet key-value store, so a wallet
//! that is built, used, and dropped hands its remaining budget to the next
//! wallet talking to the same host instead of starting full and bursting again.
//! That write is best effort: the request path never waits for the store, and a
//! dropped bucket leaves the final write to a detached task that a rebuild or a
//! runtime teardown can cut short. A caller that needs the handover to happen
//! awaits [`RateLimiterManager::flush`](crate::rate_limit::RateLimiterManager::flush)
//! (`Wallet::flush_rate_limits` in `cdk`)
//! before dropping the wallet.

use std::collections::HashMap;
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
use crate::{AuthToken, HttpError, RawResponse};

/// Namespace under which per-host rate-limit budgets are stored.
const KV_NAMESPACE: &str = "rate_limiter";

/// Database handle used to persist budgets.
type BudgetDb = Arc<dyn WalletDatabase<database::Error> + Send + Sync>;

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
        // Chosen to stay under a mint's per-minute cap (nutshell defaults to 60)
        // and to match coco's effective wallet defaults so both wallets pace a
        // shared mint identically. Over any 60s window a GCRA bucket admits at
        // most `capacity + refill` requests: one full burst plus the steady
        // trickle. So the sum, not the sustained rate alone, is what must stay
        // under the cap. 20 + 20 = 40 leaves headroom for a larger burst while
        // sustaining 20 requests/minute. The `expect`s are on compile-time
        // constant literals that are trivially non-zero.
        Self {
            capacity: NonZeroU32::new(20).expect("20 is non-zero"),
            refill_per_minute: NonZeroU32::new(20).expect("20 is non-zero"),
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

/// [`BudgetStore`] backed by the wallet key-value store, keyed by host.
#[derive(Debug)]
struct KvBudgetStore {
    db: BudgetDb,
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
    /// Time to earn one request's worth of budget.
    emission_interval: Duration,
    /// Burst window: how far the arrival time may run ahead of now before
    /// callers must wait.
    tolerance: Duration,
    /// When `false` the bucket admits every request immediately, bypassing
    /// pacing. Toggled at runtime through [`TokenBucket::set_enabled`].
    enabled: bool,
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
    state: Mutex<BucketState>,
    persistence: Option<Arc<dyn BudgetStore>>,
    /// Loads the budget once and starts the single writer, both on first use.
    started: OnceCell<Option<Writer>>,
}

/// Derive the GCRA parameters (emission interval and burst tolerance) from a
/// [`RateLimitConfig`].
fn params_from_config(config: RateLimitConfig) -> (Duration, Duration) {
    let emission_interval = Duration::from_secs(60) / config.refill_per_minute.get();
    let tolerance = emission_interval * (config.capacity.get() - 1);
    (emission_interval, tolerance)
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

    /// Create a bucket whose budget is persisted under `key`, or held only in
    /// memory when `db` is `None`.
    ///
    /// `key` is an [`origin_key`], already sanitized to the KV alphabet.
    fn persisted(config: RateLimitConfig, key: &str, db: Option<BudgetDb>) -> Self {
        let persistence = db.map(|db| {
            Arc::new(KvBudgetStore {
                db,
                key: key.to_string(),
            }) as Arc<dyn BudgetStore>
        });
        Self::build(config, persistence)
    }

    /// Create a bucket persisting through a custom [`BudgetStore`] (test seam).
    #[cfg(test)]
    fn with_store(config: RateLimitConfig, store: Arc<dyn BudgetStore>) -> Self {
        Self::build(config, Some(store))
    }

    fn build(config: RateLimitConfig, persistence: Option<Arc<dyn BudgetStore>>) -> Self {
        let (emission_interval, tolerance) = params_from_config(config);

        Self {
            inner: Arc::new(TokenBucketInner {
                state: Mutex::new(BucketState {
                    arrival_time: Instant::now(),
                    emission_interval,
                    tolerance,
                    enabled: true,
                }),
                persistence,
                started: OnceCell::new(),
            }),
        }
    }

    /// Replace the pacing configuration and enable pacing.
    ///
    /// Affects every clone that shares this bucket (main and blind-auth
    /// clients). The current budget (arrival time) is kept; only the sustained
    /// rate and burst window change.
    pub fn set_config(&self, config: RateLimitConfig) {
        let (emission_interval, tolerance) = params_from_config(config);
        let mut state = lock(&self.inner.state);
        state.emission_interval = emission_interval;
        state.tolerance = tolerance;
        state.enabled = true;
    }

    /// Enable or disable pacing at runtime.
    ///
    /// While disabled the bucket admits every request immediately and reserves
    /// nothing. Re-enabling resumes pacing from the current budget.
    pub fn set_enabled(&self, enabled: bool) {
        lock(&self.inner.state).enabled = enabled;
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
        match self.advance() {
            Some((wait, millis)) => {
                self.publish(millis);
                wait
            }
            // Pacing is disabled: admit immediately, reserve nothing.
            None => Duration::ZERO,
        }
    }

    /// Consume a slot without waiting.
    ///
    /// Returns `true` and consumes a slot if one is available within the burst
    /// window; returns `false` and reserves nothing otherwise. When pacing is
    /// disabled it always returns `true` without reserving.
    pub async fn try_acquire(&self) -> bool {
        self.ensure_started().await;
        let reserved = {
            let mut state = lock(&self.inner.state);
            if !state.enabled {
                return true;
            }
            let base = state.arrival_time.max(Instant::now());
            let ahead = base.saturating_duration_since(Instant::now());
            if ahead > state.tolerance {
                None
            } else {
                state.arrival_time = base + state.emission_interval;
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

    /// Whether the bucket carries no outstanding GCRA debt: its theoretical
    /// arrival time is at or before now, so the full burst is available again
    /// and it behaves identically to a freshly built bucket. Used to decide when
    /// a shared bucket can be dropped and rebuilt on demand without changing
    /// pacing behavior.
    fn is_fully_recovered(&self) -> bool {
        lock(&self.inner.state).arrival_time <= Instant::now()
    }

    /// Number of live handles sharing this bucket's budget. Meaningful because
    /// the background writer owns only the store and channels, never
    /// `Arc<TokenBucketInner>` (see `run_writer`), so this counts only
    /// `TokenBucket` clones, not the writer task.
    fn handle_count(&self) -> usize {
        Arc::strong_count(&self.inner)
    }

    /// Advance the theoretical arrival time by one slot under the lock. Returns
    /// the wait before the slot may be used and the new budget (wall-clock TAT
    /// in millis) to publish, so the caller need not relock the state. Returns
    /// `None` when pacing is disabled, so the caller admits without reserving.
    fn advance(&self) -> Option<(Duration, u64)> {
        let mut state = lock(&self.inner.state);
        if !state.enabled {
            return None;
        }
        let now = Instant::now();
        let base = state.arrival_time.max(now);
        let ahead = base.saturating_duration_since(now);
        let wait = ahead.saturating_sub(state.tolerance);
        state.arrival_time = base + state.emission_interval;
        Some((wait, tat_to_unix_millis(state.arrival_time)))
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

                // Read the first: seed the cache from the persisted value. The
                // load is best effort and runs on the first request's path, so a
                // hung store must not wedge it: time out and fall back to `None`
                // (start full) if the read does not complete in time.
                let loaded = match with_timeout(LOAD_TIMEOUT, store.load()).await {
                    Some(Some(bytes)) => <[u8; 8]>::try_from(bytes.as_slice())
                        .ok()
                        .map(u64::from_be_bytes),
                    _ => None,
                };
                // Seed the writer channels with the budget actually adopted in
                // memory, not the raw persisted value. A far-future or corrupt
                // stored TAT is clamped below to one burst window; seeding the
                // channels with the raw value would leave them ahead of every
                // real reservation, so the monotonic guard in `publish`/`flush`
                // would drop them all. The writer would then never persist the
                // clamped budget, `flush` would return on a false durability
                // signal, and the drop-time final write would re-persist the
                // stale far-future value, so the store would never self-heal.
                let seed = if let Some(stored) = loaded {
                    let tat_wall = UNIX_EPOCH + Duration::from_millis(stored);
                    // Clamp to the burst window so a corrupt or far-future stored
                    // value cannot wedge the wallet. The side effect is that a
                    // restart forgives any backpressure beyond one burst:
                    // inherited debt is never more than `tolerance`.
                    let mut state = lock(&self.inner.state);
                    let debt = tat_wall
                        .duration_since(SystemTime::now())
                        .unwrap_or_default()
                        .min(state.tolerance);
                    state.arrival_time = Instant::now() + debt;
                    tat_to_unix_millis(state.arrival_time)
                } else {
                    0
                };

                // Start the one writer. It owns only the store and channels (never
                // `Arc<Inner>`), so dropping the bucket closes `desired`, flushes
                // the final value, and lets the task exit.
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
    ///
    /// The update is monotonic. `advance` reserves TATs in increasing order
    /// under the state lock, but `publish` runs after that lock is released, so
    /// concurrent callers can reach here out of order. Guarding on `*current <
    /// millis` (as `flush` does) keeps a late-arriving smaller reservation from
    /// regressing the cached budget, which would otherwise persist a stale value
    /// and, on the drop-time final write, forgive debt that was already reserved.
    fn publish(&self, millis: u64) {
        if let Some(Some(writer)) = self.inner.started.get() {
            writer.desired.send_if_modified(|current| {
                if *current < millis {
                    *current = millis;
                    true
                } else {
                    false
                }
            });
        }
    }

    /// Wait until the writer has attempted to persist at least the current
    /// budget, or `FLUSH_TIMEOUT` elapses.
    ///
    /// A durability barrier for graceful shutdown (and deterministic tests): the
    /// hot path never waits for the store, but a caller can. Completion is based
    /// on the writer having *attempted* the value, so a failing store still lets
    /// `flush` return; the timeout is a backstop for a store call that never
    /// completes.
    ///
    /// Returns early when the bucket has no persistence, and when it has never
    /// been used: nothing was reserved, so there is nothing to persist and no
    /// writer to wait for. Starting one here would write a debt-free budget for
    /// an origin that never issued a request.
    pub async fn flush(&self) {
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

/// Pacing settings a manager applies to the buckets it hands out. Shared across
/// clones so a runtime change reaches every one of them.
#[derive(Debug, Clone, Copy)]
struct ManagerSettings {
    config: RateLimitConfig,
    enabled: bool,
}

/// Hands out one shared [`TokenBucket`] per destination origin.
///
/// An origin is the sanitized host plus non-default port (see [`origin_key`]).
/// Every request to one host draws down a single budget and feeds a single
/// persistence writer, no matter which wallet, currency unit, or client issued
/// it. That also keeps a wallet's non-mint traffic (LNURL, OIDC) off its mint's
/// budget. Cloning the manager shares the same per-origin map and settings
/// through `Arc`s, so cloned managers keep handing out the same live budgets.
#[derive(Clone)]
pub struct RateLimiterManager {
    settings: Arc<Mutex<ManagerSettings>>,
    /// `None` keeps every bucket in memory only, for callers with no wallet
    /// database to persist through.
    db: Option<BudgetDb>,
    /// One bucket per origin. An entry is evicted lazily (when
    /// [`Self::bucket_for`] has to create one) once its budget is fully
    /// recovered and nothing else still holds it, so the map is bounded by the
    /// origins with active or recently-active traffic. Eviction is a behavioral
    /// no-op: a recovered bucket is equivalent to a fresh one, and the persisted
    /// budget survives a re-add.
    buckets: Arc<Mutex<HashMap<String, TokenBucket>>>,
}

impl std::fmt::Debug for RateLimiterManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RateLimiterManager")
            .field("settings", &*lock(&self.settings))
            .finish_non_exhaustive()
    }
}

impl RateLimiterManager {
    /// Create a manager that hands out buckets configured with `config` and
    /// persisted through `db`, or held in memory only when `db` is `None`.
    pub fn new(config: RateLimitConfig, db: Option<BudgetDb>) -> Self {
        Self {
            settings: Arc::new(Mutex::new(ManagerSettings {
                config,
                enabled: true,
            })),
            db,
            buckets: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Get or create the shared bucket for `url`'s origin, returning a clone
    /// that draws down the same budget.
    ///
    /// The first call for an origin builds one bucket, so the KV budget and its
    /// single background writer are shared; later calls for the same origin
    /// clone it. A host-less URL has no valid KV key, so it falls back to keying
    /// on the full URL string and paces in memory without persisting.
    ///
    /// Runs on every request, so a hit is a hash lookup and a clone. Only a miss
    /// pays for the eviction sweep, which drops origins whose budget has fully
    /// recovered and that nothing else still holds: a recovered bucket is
    /// equivalent to a fresh one, so rebuilding it later is free. A bucket
    /// someone still shares (`handle_count > 1`) is kept so co-active callers
    /// never split into independent budgets.
    pub fn bucket_for(&self, url: &Url) -> TokenBucket {
        let origin = origin_key(url);
        let key = origin.clone().unwrap_or_else(|| url.to_string());
        let mut buckets = lock(&self.buckets);
        if let Some(bucket) = buckets.get(&key) {
            return bucket.clone();
        }
        buckets.retain(|_, bucket| bucket.handle_count() > 1 || !bucket.is_fully_recovered());

        let db = match origin {
            Some(_) => self.db.clone(),
            None => None,
        };
        let settings = *lock(&self.settings);
        let bucket = TokenBucket::persisted(settings.config, &key, db);
        bucket.set_enabled(settings.enabled);
        buckets.insert(key, bucket.clone());
        bucket
    }

    /// Replace the pacing configuration for every origin: the buckets already
    /// handed out and any created later.
    ///
    /// Like [`TokenBucket::set_config`] this also re-enables pacing.
    pub fn set_config(&self, config: RateLimitConfig) {
        {
            let mut settings = lock(&self.settings);
            settings.config = config;
            settings.enabled = true;
        }
        for bucket in lock(&self.buckets).values() {
            bucket.set_config(config);
        }
    }

    /// Enable or disable pacing for every origin: the buckets already handed out
    /// and any created later.
    pub fn set_enabled(&self, enabled: bool) {
        lock(&self.settings).enabled = enabled;
        for bucket in lock(&self.buckets).values() {
            bucket.set_enabled(enabled);
        }
    }

    /// Whether pacing is currently on. Reports the manager-wide toggle, which
    /// every bucket created later inherits.
    pub fn is_enabled(&self) -> bool {
        lock(&self.settings).enabled
    }

    /// Wait until every budget drawn down so far has been handed to the store.
    ///
    /// The shutdown barrier for a wallet lifecycle owner: without it, the final
    /// write is left to a detached writer task that can lose the race against an
    /// immediate wallet rebuild or a runtime teardown, and the next wallet then
    /// starts full and bursts again.
    ///
    /// Bounded even when the store is slow or hung: buckets flush concurrently
    /// and each [`TokenBucket::flush`] has its own timeout, so the whole call
    /// costs one flush timeout rather than one per origin.
    pub async fn flush(&self) {
        let buckets: Vec<TokenBucket> = lock(&self.buckets).values().cloned().collect();
        futures::future::join_all(buckets.iter().map(TokenBucket::flush)).await;
    }

    /// Number of origins currently tracked. Exposed for diagnostics and to let
    /// tests observe eviction.
    pub fn origin_count(&self) -> usize {
        lock(&self.buckets).len()
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

/// Upper bound on the one-time budget load at first use, so a hung store cannot
/// stall the first request. On timeout the bucket starts full.
const LOAD_TIMEOUT: Duration = Duration::from_millis(200);

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

/// Derive a rate-limit origin identity from a URL's host and non-default port,
/// sanitized to the KV alphabet (disallowed characters become `_`, truncated to
/// the max length). Returns `None` when the URL has no host.
///
/// This is the identity used both to key the persisted budget in the KV store
/// and to share one live in-memory [`TokenBucket`] (see [`RateLimiterManager`]).
/// The scheme is deliberately excluded, so `http://` and `https://` on one host
/// share a budget: it is one server enforcing one cap. `Url::parse` normalizes a
/// scheme-default port away, so `:443` and an implicit default map to one origin
/// too. Sanitizing can collide two distinct hosts onto one key; that only makes
/// them share a budget, never leak between them.
pub fn origin_key(url: &Url) -> Option<String> {
    let host = url.host_str()?;
    let authority = match url.port() {
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

/// A [`Transport`] decorator that paces each HTTP request through the bucket for
/// the host it is addressed to.
///
/// The bucket is resolved per request rather than fixed at construction, so a
/// transport built for a mint does not spend that mint's budget on a call to an
/// unrelated host (an LNURL service, an OIDC provider) and does not pace that
/// host by the mint's budget either.
///
/// Only the HTTP request methods are throttled; `ws_connect`, `with_proxy`, and
/// `resolve_dns_txt` pass straight through to the inner transport.
#[derive(Debug, Clone)]
pub struct RateLimitedTransport<T> {
    inner: T,
    limiter: RateLimiterManager,
}

impl<T> RateLimitedTransport<T> {
    /// Wrap `inner` with a (possibly shared) `limiter`.
    pub fn with_manager(inner: T, limiter: RateLimiterManager) -> Self {
        Self { inner, limiter }
    }
}

impl<T: Default> Default for RateLimitedTransport<T> {
    fn default() -> Self {
        Self::with_manager(
            T::default(),
            RateLimiterManager::new(RateLimitConfig::default(), None),
        )
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl<T: Transport> Transport for RateLimitedTransport<T> {
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
        let bucket = self.limiter.bucket_for(&url);
        bucket.acquire(self.inner.http_get(url, auth)).await
    }

    async fn http_get_raw(
        &self,
        url: Url,
        auth: Option<AuthToken>,
    ) -> Result<RawResponse, HttpError> {
        let bucket = self.limiter.bucket_for(&url);
        bucket.acquire(self.inner.http_get_raw(url, auth)).await
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
        let bucket = self.limiter.bucket_for(&url);
        bucket
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
        let bucket = self.limiter.bucket_for(&url);
        bucket
            .acquire(self.inner.http_post_form_raw(url, auth_token, payload))
            .await
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
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
        assert_eq!(cfg.capacity.get(), 20);
        assert_eq!(cfg.refill_per_minute.get(), 20);
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

    /// A store preloaded with a fixed budget that records every write, used to
    /// check that a far-future persisted value heals to the clamped budget.
    #[derive(Debug)]
    struct PreloadedStore {
        preload: u64,
        writes: Arc<Mutex<Vec<u64>>>,
    }

    #[async_trait]
    impl BudgetStore for PreloadedStore {
        async fn load(&self) -> Option<Vec<u8>> {
            Some(self.preload.to_be_bytes().to_vec())
        }
        async fn store(&self, value: &[u8]) -> bool {
            if let Ok(raw) = <[u8; 8]>::try_from(value) {
                lock(&self.writes).push(u64::from_be_bytes(raw));
            }
            true
        }
    }

    /// A [`BudgetStore`] that signals from its `Drop`. Because the writer task
    /// owns a clone of the store `Arc` (never `Arc<TokenBucketInner>`), the store
    /// is freed only after the task returns, so this fires exactly when the
    /// writer terminates.
    #[derive(Debug)]
    struct DropSignalStore {
        on_drop: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    }

    #[async_trait]
    impl BudgetStore for DropSignalStore {
        async fn load(&self) -> Option<Vec<u8>> {
            None
        }
        async fn store(&self, _value: &[u8]) -> bool {
            true
        }
    }

    impl Drop for DropSignalStore {
        fn drop(&mut self) {
            if let Some(tx) = lock(&self.on_drop).take() {
                let _ = tx.send(());
            }
        }
    }

    #[tokio::test]
    async fn writer_task_terminates_when_last_handle_drops() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let store: Arc<dyn BudgetStore> = Arc::new(DropSignalStore {
            on_drop: Mutex::new(Some(tx)),
        });
        let bucket = TokenBucket::with_store(config(5, 300), store);

        // First acquire spawns the writer task, which holds one store Arc clone.
        bucket.acquire(async {}).await;

        // Dropping the only handle drops Inner, closing the writer's `desired`
        // channel. The writer then does its final write, returns, and releases
        // its store Arc, firing the drop signal. This is the lifecycle eviction
        // relies on to reclaim writer tasks.
        drop(bucket);

        tokio::time::timeout(Duration::from_secs(2), rx)
            .await
            .expect("writer task should terminate and drop its store")
            .expect("drop signal sender dropped without sending");
    }

    #[tokio::test]
    async fn far_future_persisted_budget_heals_to_clamped_value() {
        // A persisted TAT an hour ahead is far beyond one burst window. The
        // bucket clamps it in memory, but it must also persist the clamped value:
        // if the writer channels were seeded with the raw far-future timestamp,
        // the monotonic guard would drop every reservation and the store would
        // stay wedged at the hour-ahead value.
        let now_millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("after epoch")
            .as_millis() as u64;
        let far_future = now_millis + 3_600_000;
        let writes = Arc::new(Mutex::new(Vec::new()));
        let store = Arc::new(PreloadedStore {
            preload: far_future,
            writes: writes.clone(),
        });
        // capacity 5, refill 300/min => tolerance ~800ms, so the clamped budget
        // sits at most ~1s ahead of now, far below the hour-ahead persisted value.
        let bucket = TokenBucket::with_store(config(5, 300), store);

        bucket.acquire(async {}).await;
        bucket.flush().await;

        let writes = lock(&writes).clone();
        assert!(!writes.is_empty(), "the clamped budget must be persisted");
        let max_written = writes.iter().copied().max().expect("non-empty");
        assert!(
            max_written < far_future,
            "persisted budget {max_written} should heal below the far-future seed {far_future}",
        );
        // What was persisted stays within a burst window of now, not an hour out.
        assert!(max_written <= now_millis + 60_000);
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

    /// A [`BudgetStore`] whose writes never complete, so `flush` can only return
    /// through its own timeout.
    #[derive(Debug)]
    struct HangingStore;

    #[async_trait]
    impl BudgetStore for HangingStore {
        async fn load(&self) -> Option<Vec<u8>> {
            None
        }
        async fn store(&self, _value: &[u8]) -> bool {
            std::future::pending().await
        }
    }

    /// Paused time auto-advances while every task is idle, so the flush timeout
    /// elapses in virtual time: this asserts the bound exists, without waiting
    /// five real seconds for it.
    #[tokio::test(start_paused = true)]
    async fn flush_is_bounded_when_the_store_hangs() {
        let bucket = TokenBucket::with_store(config(5, 300), Arc::new(HangingStore));
        bucket.acquire(async {}).await;

        assert!(
            with_timeout(FLUSH_TIMEOUT * 2, bucket.flush())
                .await
                .is_some(),
            "flush must give up rather than wait on a hung store",
        );
    }

    /// No request ever went through, so there is nothing to hand over and no
    /// writer to wait for: a manager flushing every origin must not write a
    /// debt-free budget for one that never issued a request.
    #[tokio::test]
    async fn flush_on_an_unused_bucket_persists_nothing() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let store = Arc::new(PreloadedStore {
            preload: 0,
            writes: writes.clone(),
        });
        let bucket = TokenBucket::with_store(config(5, 300), store);

        bucket.flush().await;
        assert!(lock(&writes).is_empty());
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

    fn parse(url: &str) -> Url {
        Url::parse(url).expect("valid url")
    }

    #[test]
    fn kv_key_is_sanitized() {
        let key = origin_key(&parse("https://mint.example.com:3338")).expect("host present");
        assert!(!key.contains('.'));
        assert!(!key.contains(':'));
        assert!(key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));
    }

    #[test]
    fn kv_key_ignores_default_port() {
        // `Url::parse` normalizes a scheme-default port away, so an explicit
        // `:443` and an implicit default map to one budget for the same host.
        let implicit = origin_key(&parse("https://mint.example.com"));
        let explicit = origin_key(&parse("https://mint.example.com:443"));
        assert_eq!(implicit, explicit);
        assert_eq!(implicit.as_deref(), Some("mint_example_com"));

        // A non-default port still yields a distinct budget.
        let other = origin_key(&parse("https://mint.example.com:8443"));
        assert_ne!(implicit, other);
    }

    #[test]
    fn kv_key_ignores_scheme_and_path() {
        // One host is one server enforcing one cap, whatever the scheme, and
        // whatever path a request targets.
        let plain = origin_key(&parse("http://mint.example.com/v1/keys"));
        let secure = origin_key(&parse("https://mint.example.com/other/mint"));
        assert_eq!(plain, secure);
    }

    #[tokio::test]
    async fn manager_paces_each_origin_independently() {
        let manager = RateLimiterManager::new(config(1, 300), None);
        let mint = manager.bucket_for(&parse("https://mint.example.com/v1/mint"));
        // A different path at the same host resolves to the same budget.
        let same_host = manager.bucket_for(&parse("https://mint.example.com/v1/melt"));
        // An unrelated host (an LNURL service, say) gets its own.
        let lnurl = manager.bucket_for(&parse("https://pay.example.org/.well-known/lnurlp/alice"));

        assert!(mint.try_acquire().await);
        assert!(!same_host.try_acquire().await, "same host shares a budget");
        assert!(lnurl.try_acquire().await, "another host is independent");
    }

    #[tokio::test]
    async fn manager_toggles_reach_existing_and_future_buckets() {
        let manager = RateLimiterManager::new(config(1, 60), None);
        let existing = manager.bucket_for(&parse("https://mint.example.com"));
        assert!(existing.try_acquire().await);
        assert!(!existing.try_acquire().await);

        manager.set_enabled(false);
        assert!(existing.try_acquire().await, "existing bucket is disabled");
        let later = manager.bucket_for(&parse("https://pay.example.org"));
        for _ in 0..5 {
            assert!(later.try_acquire().await, "new bucket inherits disabled");
        }

        // set_config re-enables pacing everywhere. The existing bucket keeps the
        // debt it had before being disabled, so it is paced again immediately.
        manager.set_config(config(1, 60));
        assert!(!existing.try_acquire().await, "existing bucket paces again");
        let newest = manager.bucket_for(&parse("https://third.example.net"));
        assert!(newest.try_acquire().await);
        assert!(!newest.try_acquire().await, "new bucket inherits config");
    }

    #[tokio::test]
    async fn manager_reports_whether_pacing_is_on() {
        let manager = RateLimiterManager::new(config(1, 60), None);
        assert!(manager.is_enabled(), "a fresh manager paces");

        manager.set_enabled(false);
        assert!(!manager.is_enabled());

        manager.set_config(config(2, 120));
        assert!(manager.is_enabled(), "set_config re-enables pacing");
    }

    #[tokio::test]
    async fn manager_evicts_only_recovered_unheld_origins() {
        let manager = RateLimiterManager::new(config(1, 60), None);
        // Held by the test, so it survives an eviction sweep even once
        // recovered.
        let held = manager.bucket_for(&parse("https://held.example.com"));
        // Dropped immediately and never used, so it is fully recovered and
        // unheld: the next miss sweeps it away.
        manager.bucket_for(&parse("https://stale.example.com"));
        assert_eq!(manager.origin_count(), 2);

        manager.bucket_for(&parse("https://fresh.example.com"));
        assert_eq!(manager.origin_count(), 2, "stale origin was evicted");
        // The held origin kept its drawn-down budget rather than being rebuilt.
        assert!(held.try_acquire().await);
        assert!(!held.try_acquire().await);
        assert!(
            !manager
                .bucket_for(&parse("https://held.example.com"))
                .try_acquire()
                .await
        );
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
    async fn disabled_bucket_admits_immediately() {
        // capacity 1 => the second acquire would normally block.
        let bucket = TokenBucket::new(config(1, 60));
        bucket.set_enabled(false);
        let start = StdInstant::now();
        for _ in 0..10 {
            bucket.acquire(async {}).await;
        }
        assert!(start.elapsed() < Duration::from_millis(100));
        // try_acquire also always admits while disabled.
        assert!(bucket.try_acquire().await);
        assert!(bucket.try_acquire().await);
    }

    #[tokio::test]
    async fn re_enabling_resumes_pacing() {
        let bucket = TokenBucket::new(config(1, 300));
        bucket.set_enabled(false);
        // Drain freely while disabled.
        for _ in 0..5 {
            bucket.acquire(async {}).await;
        }
        // Re-enable with a fresh config; the first burst slot is free, the next
        // one must wait for the ~200ms emission interval.
        bucket.set_config(config(1, 300));
        bucket.acquire(async {}).await;
        let start = StdInstant::now();
        bucket.acquire(async {}).await;
        assert!(start.elapsed() >= Duration::from_millis(150));
    }

    #[tokio::test]
    async fn set_config_changes_rate() {
        // Start slow (capacity 1), then widen the burst so several go through.
        let bucket = TokenBucket::new(config(1, 60));
        bucket.set_config(config(5, 600));
        let start = StdInstant::now();
        for _ in 0..5 {
            bucket.acquire(async {}).await;
        }
        assert!(start.elapsed() < Duration::from_millis(100));
    }

    #[tokio::test]
    async fn set_config_re_enables_a_disabled_bucket() {
        let bucket = TokenBucket::new(config(2, 600));
        bucket.set_enabled(false);
        // set_config turns pacing back on, so the burst is enforced again.
        bucket.set_config(config(2, 600));
        assert!(bucket.try_acquire().await);
        assert!(bucket.try_acquire().await);
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
        let transport = RateLimitedTransport::with_manager(
            inner,
            RateLimiterManager::new(config(2, 300), None),
        );

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
    async fn transport_paces_by_destination_host() {
        // The whole point of resolving per request: a transport built for a mint
        // must not spend the mint's budget on an LNURL or OIDC host.
        let transport = RateLimitedTransport::with_manager(
            CountingTransport::default(),
            RateLimiterManager::new(config(1, 300), None),
        );
        let mint = parse("https://mint.example.com/v1/keys");
        let lnurl = parse("https://pay.example.org/.well-known/lnurlp/alice");

        // Drain the mint's single burst slot.
        let _ = transport.http_get_raw(mint.clone(), None).await;

        // The unrelated host still has its own full burst.
        let start = StdInstant::now();
        let _ = transport.http_get_raw(lnurl, None).await;
        assert!(
            start.elapsed() < Duration::from_millis(100),
            "another host must not be paced by the mint's budget"
        );

        // The mint's own budget is still drawn down.
        let start = StdInstant::now();
        let _ = transport.http_get_raw(mint, None).await;
        assert!(
            start.elapsed() >= Duration::from_millis(150),
            "the mint's own budget is still enforced"
        );
    }

    #[tokio::test]
    async fn transport_passes_proxy_through_unthrottled() {
        let inner = CountingTransport::default();
        let flag = inner.proxied.clone();
        let counter = inner.clone();
        let mut transport = RateLimitedTransport::with_manager(
            inner,
            RateLimiterManager::new(config(1, 300), None),
        );

        transport.with_proxy(url(), None, false).expect("proxy set");
        // with_proxy reached the inner transport and consumed no rate-limit slot.
        assert!(flag.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(counter.http_calls(), 0);
    }

    #[tokio::test]
    async fn transports_sharing_a_manager_share_the_budget() {
        let manager = RateLimiterManager::new(config(1, 300), None);
        let a = RateLimitedTransport::with_manager(CountingTransport::default(), manager.clone());
        let b = RateLimitedTransport::with_manager(CountingTransport::default(), manager.clone());

        // Drain the single burst slot through transport A.
        let _ = a.http_get_raw(url(), None).await;

        // Transport B, sharing the same manager and so the same per-host bucket,
        // must now wait.
        let start = StdInstant::now();
        let _ = b.http_get_raw(url(), None).await;
        assert!(
            start.elapsed() >= Duration::from_millis(150),
            "shared bucket should force B to pace"
        );
    }
}

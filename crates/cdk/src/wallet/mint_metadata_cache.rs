//! Per-mint cryptographic key and metadata cache
//!
//! Provides on-demand fetching and caching of mint metadata (info, keysets, and keys)
//! with atomic in-memory cache updates and database persistence.
//!
//! # Architecture
//!
//! - **Pull-based loading**: Keys fetched on-demand from mint HTTP API
//! - **Atomic cache**: Single `MintMetadata` snapshot updated via `ArcSwap`
//! - **Synchronous persistence**: Database writes happen after cache update
//! - **Multi-database support**: Tracks sync status per storage instance via pointer identity
//!
//! # Usage
//!
//! ```ignore
//! // Create manager (cheap, no I/O)
//! let manager = Arc::new(MintMetadataCache::new(mint_url));
//!
//! // Load metadata (returns cached if available, fetches if not)
//! let metadata = manager.load(&storage, &client).await?;
//! let keys = metadata.keys.get(&keyset_id).ok_or(Error::UnknownKeySet)?;
//!
//! // Force refresh from mint
//! let fresh = manager.load_from_mint(&storage, &client).await?;
//! ```

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::{Arc, Weak};
use std::time::Duration;

use arc_swap::ArcSwap;
use cdk_common::database::{self, WalletDatabase};
use cdk_common::mint_url::MintUrl;
use cdk_common::nuts::{KeySetInfo, Keys};
use cdk_common::parking_lot::RwLock;
use cdk_common::{CurrencyUnit, KeySet, MintInfo};
use tokio::sync::Mutex;
use web_time::Instant;

use crate::nuts::Id;
use crate::wallet::util::escape_log_value;
use crate::wallet::{AuthMintConnector, AuthWallet, MintConnector};
use crate::{Error, Wallet};

/// Metadata freshness and versioning information
///
/// Tracks when data was last fetched and which version is currently cached.
/// Used to determine if cache is ready and if database sync is needed.
#[derive(Clone, Debug)]
pub struct FreshnessStatus {
    /// Whether this data has been successfully fetched at least once
    pub is_populated: bool,

    /// A future time when the cache would be considered as staled.
    pub updated_at: Instant,

    /// Monotonically increasing version number (for database sync tracking)
    version: usize,
}

impl Default for FreshnessStatus {
    fn default() -> Self {
        Self {
            is_populated: false,
            updated_at: Instant::now(),
            version: 0,
        }
    }
}

/// Complete metadata snapshot for a single mint
///
/// Contains all cryptographic keys, keyset metadata, and mint information
/// fetched from a mint server. This struct is atomically swapped as a whole
/// to ensure readers always see a consistent view.
///
/// Cloning is cheap due to `Arc` wrapping of large data structures.
#[derive(Clone, Debug, Default)]
pub struct MintMetadata {
    /// Mint server information, `None` until a fetch or a database read has
    /// supplied it.
    pub mint_info: Option<MintInfo>,

    /// All keysets indexed by their ID (includes both active and inactive)
    pub keysets: HashMap<Id, Arc<KeySetInfo>>,

    /// Cryptographic keys for each keyset, indexed by keyset ID
    pub keys: HashMap<Id, Arc<Keys>>,

    /// Freshness tracking for regular (non-auth) mint data
    pub(crate) status: FreshnessStatus,

    /// Freshness tracking for blind auth keysets
    auth_status: FreshnessStatus,
}

/// Snapshot versions a database has been written up to, one per data kind.
///
/// Compared as a pair because a persist writes the whole snapshot, so a single
/// counter cannot say what a database holds. Neither component ever decreases,
/// so the derived lexicographic order agrees with "at least as new as".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct SnapshotVersions {
    status: usize,
    auth: usize,
}

/// First wait a database earns by rejecting a write, doubled on each further
/// rejection up to [`MAX_PERSIST_RETRY_DELAY`].
const FIRST_PERSIST_RETRY_DELAY: Duration = Duration::from_secs(1);

/// Longest wait between retries of a rejected persist.
const MAX_PERSIST_RETRY_DELAY: Duration = Duration::from_secs(60);

/// How long a database that rejected a write is left alone.
///
/// Bounds the retry `load` would otherwise make on every call for as long as a
/// snapshot stays unpersisted. A snapshot newer than `failed` is written as
/// soon as it exists, because what produces one is already rate limited by the
/// TTL.
struct Backoff {
    /// Snapshot whose persist was rejected.
    failed: SnapshotVersions,

    /// When a persist of `failed` or older may run again.
    next_attempt: Instant,

    /// Wait this backoff was given, which the next rejection doubles.
    delay: Duration,
}

/// What one database has been written up to, with a handle that keeps its
/// address from being reused.
///
/// The map is keyed by the storage `Arc`'s address, so a record has to outlive
/// the strong pointers it was made for: the `Weak` holds the allocation open,
/// which stops a later database from being allocated on top of a stale record.
struct SyncRecord {
    storage: Weak<dyn WalletDatabase<database::Error> + Send + Sync>,

    /// `None` while nothing has been written to this database, which is not
    /// the same as version zero: a snapshot read out of another database stays
    /// at zero and still has to be persisted.
    versions: Option<SnapshotVersions>,

    /// Set while this database is being left alone after rejecting a write.
    backoff: Option<Backoff>,
}

/// How a persist ended, which decides what its claim does on the way out.
enum ClaimOutcome {
    /// The caller was dropped before the persist finished.
    Cancelled,

    /// Every write succeeded.
    Kept,

    /// The database rejected at least one write.
    Rejected,
}

/// A claimed snapshot version, given back unless the persist holding it wrote
/// everything.
///
/// The claim is taken before the writes start, so a concurrent persist of the
/// same snapshot does not duplicate them, and released from `Drop` rather than
/// at the end of the persist, because a caller dropped mid-write never reaches
/// that point and would leave the snapshot recorded as stored.
struct SyncClaim {
    db_sync_versions: Arc<RwLock<HashMap<usize, SyncRecord>>>,
    storage_id: usize,
    claimed: SnapshotVersions,
    previous: Option<SnapshotVersions>,
    outcome: ClaimOutcome,
}

impl SyncClaim {
    /// Records `versions` against `storage`, or returns `None` when that
    /// database already holds them or is still inside the backoff a rejected
    /// write earned it.
    fn take(
        db_sync_versions: Arc<RwLock<HashMap<usize, SyncRecord>>>,
        storage: &Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
        versions: SnapshotVersions,
    ) -> Option<Self> {
        let storage_id = MintMetadataCache::arc_pointer_id(storage);

        let previous = {
            let mut records = db_sync_versions.write();

            records.retain(|_, record| record.storage.strong_count() > 0);

            match records.entry(storage_id) {
                Entry::Occupied(mut entry) => {
                    let record = entry.get_mut();

                    if record.versions.is_some_and(|held| versions <= held) {
                        return None;
                    }

                    if record.backoff.as_ref().is_some_and(|backoff| {
                        versions <= backoff.failed && Instant::now() < backoff.next_attempt
                    }) {
                        return None;
                    }

                    record.versions.replace(versions)
                }
                Entry::Vacant(entry) => {
                    entry.insert(SyncRecord {
                        storage: Arc::downgrade(storage),
                        versions: Some(versions),
                        backoff: None,
                    });

                    None
                }
            }
        };

        Some(Self {
            db_sync_versions,
            storage_id,
            claimed: versions,
            previous,
            outcome: ClaimOutcome::Cancelled,
        })
    }

    /// Keeps the claim, which only a persist whose every write succeeded has
    /// earned.
    fn keep(mut self) {
        self.outcome = ClaimOutcome::Kept;
    }

    /// Gives the claim back and leaves the database alone for a while, after
    /// it rejected a write.
    fn reject(mut self) {
        self.outcome = ClaimOutcome::Rejected;
    }
}

impl Drop for SyncClaim {
    fn drop(&mut self) {
        let mut records = self.db_sync_versions.write();

        let Some(record) = records.get_mut(&self.storage_id) else {
            return;
        };

        if let ClaimOutcome::Kept = self.outcome {
            if record
                .backoff
                .as_ref()
                .is_some_and(|backoff| backoff.failed <= self.claimed)
            {
                record.backoff = None;
            }

            return;
        }

        if record.versions != Some(self.claimed) {
            return;
        }

        record.versions = self.previous;

        if let ClaimOutcome::Rejected = self.outcome {
            let delay = record
                .backoff
                .as_ref()
                .map_or(FIRST_PERSIST_RETRY_DELAY, |backoff| {
                    (backoff.delay * 2).min(MAX_PERSIST_RETRY_DELAY)
                });

            record.backoff = Some(Backoff {
                failed: self.claimed,
                next_attempt: Instant::now() + delay,
                delay,
            });
        }
    }
}

impl MintMetadata {
    /// Calculate input fees from keyset information, even without cached keys.
    pub(crate) fn proofs_fee(
        &self,
        proofs_per_keyset: &HashMap<Id, u64>,
    ) -> Result<crate::fees::ProofsFeeBreakdown, Error> {
        let fee_rates = proofs_per_keyset
            .keys()
            .map(|id| {
                self.keysets
                    .get(id)
                    .map(|info| (*id, info.input_fee_ppk))
                    .ok_or(Error::UnknownKeySet)
            })
            .collect::<Result<HashMap<_, _>, _>>()?;
        crate::fees::calculate_fee(proofs_per_keyset, &fee_rates)
    }

    /// Both freshness counters, for comparing against what a database holds.
    fn versions(&self) -> SnapshotVersions {
        SnapshotVersions {
            status: self.status.version,
            auth: self.auth_status.version,
        }
    }

    /// Keysets the mint is currently signing with.
    ///
    /// Derived on call rather than stored, because two writers merging into
    /// the same snapshot would each rebuild a stored list from their own
    /// partial view.
    pub fn active_keysets(&self) -> Vec<Arc<KeySetInfo>> {
        self.keysets
            .values()
            .filter(|keyset| keyset.active)
            .cloned()
            .collect()
    }
}

/// On-demand mint metadata cache with database persistence
///
/// Manages a single mint's cryptographic keys and metadata. Fetches data from
/// the mint's HTTP API on-demand and caches it in memory. Database writes
/// occur synchronously to ensure persistence.
///
/// # Thread Safety
///
/// All methods are safe to call concurrently. The cache uses `ArcSwap` for
/// lock-free reads and atomic updates. A `Mutex` ensures only one fetch
/// operation runs at a time, with other callers waiting and re-reading cache.
///
/// # Cloning
///
/// Cheap to clone - all data is behind `Arc`. Clones share the same cache.
#[derive(Clone)]
pub struct MintMetadataCache {
    /// The mint server URL this cache manages
    mint_url: MintUrl,

    /// Atomically-updated metadata snapshot (lock-free reads)
    metadata: Arc<ArcSwap<MintMetadata>>,

    /// How long cached metadata is considered fresh before re-fetching.
    /// `None` means cached data never expires (unless manually refreshed).
    /// Default: 1 hour (3600 seconds).
    ttl: Arc<RwLock<Option<Duration>>>,

    /// What each database has been written up to, keyed by the pointer
    /// identity of its storage `Arc`.
    ///
    /// A missing entry means nothing is known about that database at all;
    /// a record is also what carries the backoff a rejected write earns.
    db_sync_versions: Arc<RwLock<HashMap<usize, SyncRecord>>>,

    /// Mutex to ensure only one fetch operation runs at a time
    /// Other callers wait for the lock, then re-read the updated cache
    fetch_lock: Arc<Mutex<()>>,
}

impl std::fmt::Debug for MintMetadataCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MintMetadataCache")
            .field("mint_url", &self.mint_url)
            .field("is_populated", &self.metadata.load().status.is_populated)
            .field("keyset_count", &self.metadata.load().keysets.len())
            .finish()
    }
}

impl Wallet {
    /// Sets the metadata cache TTL.
    ///
    /// The TTL determines how often the wallet re-fetches keysets and mint info.
    /// Because the cache is shared, this affects all wallets using the same mint.
    ///
    /// `None` means cached data never expires. Default: 1 hour.
    pub fn set_metadata_cache_ttl(&self, ttl: Option<Duration>) {
        self.metadata_cache.set_ttl(ttl);
    }

    /// Get information about metadata cache info
    pub fn get_metadata_cache_info(&self) -> FreshnessStatus {
        self.metadata_cache.metadata.load().status.clone()
    }
}

impl AuthWallet {
    /// Get information about metadata cache info
    pub fn get_metadata_cache_info(&self) -> FreshnessStatus {
        self.metadata_cache.metadata.load().auth_status.clone()
    }
}

impl MintMetadataCache {
    /// Compute a unique identifier for an Arc pointer
    ///
    /// Used to track which storage instances have been synced. We use pointer
    /// identity rather than a counter because wallets may use multiple storage
    /// backends simultaneously (e.g., different databases for different mints).
    /// Each record keeps a `Weak` to its `Arc`, because an address the
    /// allocator recycles would otherwise let a new database inherit the sync
    /// state of a dropped one.
    fn arc_pointer_id<T>(arc: &Arc<T>) -> usize
    where
        T: ?Sized,
    {
        Arc::as_ptr(arc) as *const () as usize
    }

    /// Create a new metadata cache for the given mint
    ///
    /// This is a cheap operation that only allocates memory. No network or
    /// database I/O occurs until `load()` or `load_from_mint()` is called.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let cache = MintMetadataCache::new(mint_url);
    /// // No data loaded yet - call load() to fetch
    /// ```
    pub fn new(mint_url: MintUrl) -> Self {
        Self {
            mint_url,
            metadata: Arc::new(ArcSwap::default()),
            ttl: Arc::new(RwLock::new(Some(Duration::from_secs(3600)))),
            db_sync_versions: Arc::new(Default::default()),
            fetch_lock: Arc::new(Mutex::new(())),
        }
    }

    /// Set the TTL for cached metadata.
    ///
    /// `None` means cached data never expires.
    pub fn set_ttl(&self, ttl: Option<Duration>) {
        *self.ttl.write() = ttl;
    }

    /// Get the current TTL.
    pub fn ttl(&self) -> Option<Duration> {
        *self.ttl.read()
    }

    /// Return cached metadata if populated, without fetching or TTL checks.
    pub fn get_cached(&self) -> Option<Arc<MintMetadata>> {
        let m = self.metadata.load().clone();
        if m.status.is_populated {
            Some(m)
        } else {
            None
        }
    }

    /// Load metadata from in-memory cache or database, without network access.
    ///
    /// Checks the in-memory cache first. If not populated, loads from the
    /// database. Returns an error if neither source has data.
    pub async fn load_cached(
        &self,
        storage: &Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
    ) -> Result<Arc<MintMetadata>, Error> {
        if let Some(cached) = self.get_cached() {
            return Ok(cached);
        }

        let from_db = self.load_from_db(storage).await?;
        if from_db.status.is_populated {
            return Ok(from_db);
        }

        Err(Error::UnknownKeySet)
    }

    /// Load metadata from mint server and update cache
    ///
    /// Always performs an HTTP fetch from the mint server to get fresh data.
    /// Updates the in-memory cache and persists to the database.
    ///
    /// Uses a mutex to ensure only one fetch runs at a time. If multiple
    /// callers request a fetch simultaneously, only one performs the HTTP
    /// request while others wait for the lock, then return the updated cache.
    ///
    /// Waiting for that lock returns early only when another caller fetched the
    /// regular mint data. `status.version` moves for no other reason, so
    /// neither a database read nor an auth fetch can downgrade a forced refresh
    /// to a local read.
    ///
    /// Use this when you need guaranteed fresh data from the mint.
    ///
    /// A database read is attempted first only to seed keys the fetch can then
    /// skip, so its failure is logged and ignored: the mint is about to supply
    /// everything anyway.
    ///
    /// # Arguments
    ///
    /// * `storage` - Database the fetched metadata is written to before returning
    /// * `client` - HTTP client for fetching from mint server
    ///
    /// # Returns
    ///
    /// Fresh metadata from the mint server
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Force refresh from mint (ignores cache)
    /// let fresh = cache.load_from_mint(&storage, &client).await?;
    /// ```
    pub async fn load_from_mint(
        &self,
        storage: &Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
        client: &Arc<dyn MintConnector + Send + Sync>,
    ) -> Result<Arc<MintMetadata>, Error> {
        // Acquire lock to ensure only one fetch at a time
        let current_version = self.metadata.load().status.version;
        let _guard = self.fetch_lock.lock().await;

        let current_metadata = self.metadata.load().clone();
        if current_metadata.status.is_populated && current_metadata.status.version > current_version
        {
            // Cache was just updated by another caller - return it
            tracing::debug!(
                "Cache was updated while waiting for fetch lock, returning cached data"
            );
            return Ok(current_metadata);
        }

        if !current_metadata.status.is_populated {
            self.load_from_db(storage)
                .await
                .inspect_err(|err| {
                    tracing::debug!(
                        "No usable database metadata for {} before fetching: {}",
                        self.mint_url,
                        err
                    )
                })
                .ok();
        }

        // Perform the fetch
        // Note: keys already in cache (e.g. from load_from_db at boot) are
        // skipped by fetch_from_http's Vacant entry check.
        let metadata = self.fetch_from_http(Some(client), None).await?;

        // Persist to database
        self.database_sync(storage.clone(), metadata.clone()).await;

        Ok(metadata)
    }

    /// Load metadata from database and update cache
    ///
    /// Populates the in-memory cache from persisted database data without
    /// making any HTTP requests. Useful for offline scenarios or fast startup
    /// when the wallet can work with previously-fetched data.
    ///
    /// Reads every row first, then merges once, so nothing is held across the
    /// database round trips. The merge only fills keysets the snapshot lacks:
    /// the mint is authoritative, so a read landing after a concurrent fetch
    /// must not put an older `final_expiry` back.
    ///
    /// Leaves `status.version` and `updated_at` untouched, because data out of a
    /// database is not a new version and claiming it was made a concurrent
    /// fetch look already persisted. The database read from is recorded at
    /// version zero instead, so the snapshot it supplied is not written
    /// straight back to it on the next load, which would cost a round trip per
    /// keyset and could undo a fresher write by another wallet on the same
    /// database. Any later fetch rises above zero and so still reaches it.
    ///
    /// Blind auth keysets do not populate the regular status: a database
    /// holding only those would otherwise make `load` serve a snapshot with no
    /// keyset for the wallet's unit until the TTL expires.
    ///
    /// # Arguments
    ///
    /// * `storage` - Database to load metadata from
    ///
    /// # Returns
    ///
    /// Metadata loaded from the database
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Load from database (no HTTP)
    /// let metadata = cache.load_from_db(&storage).await?;
    /// ```
    async fn load_from_db(
        &self,
        storage: &Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
    ) -> Result<Arc<MintMetadata>, Error> {
        let mint_info = storage.get_mint(self.mint_url.clone()).await?;

        let stored = storage
            .get_mint_keysets(self.mint_url.clone())
            .await?
            .unwrap_or_default();

        let mut stored_keys = HashMap::with_capacity(stored.len());
        for keyset_info in &stored {
            if let Some(keys) = storage.get_keys(&keyset_info.id).await? {
                tracing::trace!("Loaded keys for keyset {} from database", keyset_info.id);
                stored_keys.insert(keyset_info.id, Arc::new(keys));
            }
        }

        self.metadata.rcu(|current| {
            let mut next = (**current).clone();

            if let Some(mint_info) = mint_info.clone() {
                next.mint_info = Some(mint_info);
            }

            for keyset_info in &stored {
                next.keysets
                    .entry(keyset_info.id)
                    .or_insert_with(|| Arc::new(keyset_info.clone()));
            }

            for (id, keys) in &stored_keys {
                next.keys.entry(*id).or_insert_with(|| keys.clone());
            }

            next.status.is_populated = next
                .keysets
                .values()
                .any(|keyset| keyset.unit != CurrencyUnit::Auth);

            Arc::new(next)
        });

        let metadata = self.metadata.load().clone();

        {
            let storage_id = Self::arc_pointer_id(storage);
            let mut versions = self.db_sync_versions.write();

            versions.retain(|_, record| record.storage.strong_count() > 0);

            let record = versions.entry(storage_id).or_insert_with(|| SyncRecord {
                storage: Arc::downgrade(storage),
                versions: None,
                backoff: None,
            });

            if record.versions.is_none() {
                record.versions = Some(SnapshotVersions::default());
            }
        }

        tracing::info!(
            "Loaded cache from database for {} with {} keysets (version {})",
            self.mint_url,
            metadata.keysets.len(),
            metadata.status.version
        );

        Ok(metadata)
    }

    /// Load metadata from cache or fetch if not available
    ///
    /// Returns cached metadata if available and it is still valid, otherwise fetches from the mint.
    /// A database behind the cache is written before returning.
    ///
    /// This is the primary method for normal operations - it balances freshness
    /// with performance by returning cached data when available.
    ///
    /// # Arguments
    ///
    /// * `storage` - Database to persist metadata to (if fetched or stale)
    /// * `client` - HTTP client for fetching from mint (only if cache empty)
    ///
    /// # Returns
    ///
    /// Metadata from cache if available, otherwise fresh from mint
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Use cached data if available, fetch if not
    /// let metadata = cache.load(&storage, &client).await?;
    /// ```
    pub async fn load(
        &self,
        storage: &Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
        client: &Arc<dyn MintConnector + Send + Sync>,
    ) -> Result<Arc<MintMetadata>, Error> {
        let cached_metadata = self.metadata.load().clone();
        let storage_id = Self::arc_pointer_id(storage);
        let ttl = *self.ttl.read();

        // Check what version of cache this database has seen
        let db_synced_version = self
            .db_sync_versions
            .read()
            .get(&storage_id)
            .and_then(|record| record.versions);

        if cached_metadata.status.is_populated
            && ttl
                .map(|ttl| cached_metadata.status.updated_at + ttl > Instant::now())
                .unwrap_or(true)
        {
            // Cache is ready - check if database needs updating
            if db_synced_version != Some(cached_metadata.versions()) {
                // Database is stale - sync before returning
                self.database_sync(storage.clone(), cached_metadata.clone())
                    .await;
            }
            return Ok(cached_metadata);
        }

        // In-memory cache was empty (not just stale) — try database before network
        if !cached_metadata.status.is_populated {
            match self.load_from_db(storage).await {
                Ok(metadata) if metadata.status.is_populated => return Ok(metadata),
                Ok(_) => {}
                Err(err) => tracing::debug!(
                    "No usable database metadata for {} before fetching: {}",
                    self.mint_url,
                    err
                ),
            }
        }

        // Cache was stale (TTL expired) or neither cache nor DB had data — fetch from mint
        match self.load_from_mint(storage, client).await {
            Ok(metadata) => Ok(metadata),
            Err(e) if cached_metadata.status.is_populated => {
                // Network failed but we have usable stale data — return it
                tracing::warn!(
                    "Failed to refresh metadata from mint {}, using stale cache: {}",
                    self.mint_url,
                    e
                );
                Ok(cached_metadata)
            }
            Err(e) => Err(e),
        }
    }

    /// Load auth keysets and keys (auth feature only)
    ///
    /// Returns cached blind authentication keysets if they are populated and
    /// still fresh according to `ttl`; otherwise fetches them from the mint over
    /// HTTP and updates the cache.
    ///
    /// # Arguments
    ///
    /// * `storage` - Database to persist metadata to
    /// * `auth_client` - Auth-capable HTTP client for fetching blind auth keysets
    ///
    /// # Returns
    ///
    /// Metadata containing auth keysets and keys
    pub async fn load_auth(
        &self,
        storage: &Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
        auth_client: &Arc<dyn AuthMintConnector + Send + Sync>,
    ) -> Result<Arc<MintMetadata>, Error> {
        let cached_metadata = self.metadata.load().clone();
        let storage_id = Self::arc_pointer_id(storage);
        let ttl = *self.ttl.read();

        let db_synced_version = self
            .db_sync_versions
            .read()
            .get(&storage_id)
            .and_then(|record| record.versions);

        // Check if auth data is populated in cache and still fresh
        if cached_metadata.auth_status.is_populated
            && ttl
                .map(|ttl| cached_metadata.auth_status.updated_at + ttl > Instant::now())
                .unwrap_or(true)
        {
            if db_synced_version != Some(cached_metadata.versions()) {
                // Database needs updating - sync before returning
                self.database_sync(storage.clone(), cached_metadata.clone())
                    .await;
            }
            return Ok(cached_metadata);
        }

        // Acquire fetch lock to ensure only one auth fetch at a time
        let _guard = self.fetch_lock.lock().await;

        // Re-check if auth data was updated while waiting for lock
        let current_metadata = self.metadata.load().clone();
        if current_metadata.auth_status.is_populated
            && ttl
                .map(|ttl| current_metadata.auth_status.updated_at + ttl > Instant::now())
                .unwrap_or(true)
        {
            tracing::debug!(
                "Auth cache was updated while waiting for fetch lock, returning cached data"
            );
            return Ok(current_metadata);
        }

        // Auth data not in cache - fetch from mint
        let metadata = self.fetch_from_http(None, Some(auth_client)).await?;

        // Persist to database
        self.database_sync(storage.clone(), metadata.clone()).await;

        Ok(metadata)
    }

    /// Sync metadata to database
    ///
    /// This will:
    /// 1. Check if this sync is still needed (version may be superseded)
    /// 2. Save mint info, keysets, and keys to the database
    /// 3. Update the sync tracking to record this storage has been updated
    async fn database_sync(
        &self,
        storage: Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
        metadata: Arc<MintMetadata>,
    ) {
        let mint_url = self.mint_url.clone();
        let db_sync_versions = self.db_sync_versions.clone();

        Self::persist_to_database(mint_url, storage, metadata, db_sync_versions).await
    }

    /// Persist metadata to database
    ///
    /// Saves mint info, keysets, and keys to the database. Checks version
    /// before writing to avoid redundant work if a newer version has already
    /// been persisted. The version is claimed before the writes start, so a
    /// concurrent persist of the same snapshot does not duplicate them. A
    /// database with no record of its own is always written to, because a
    /// snapshot read out of another database never leaves version zero; the
    /// database it was read from records that zero itself.
    ///
    /// A snapshot carrying no regular data is not written at all, because the
    /// mint row its keysets need may not exist yet and the first regular
    /// persist carries the auth keysets anyway.
    ///
    /// Mint info is written only when the snapshot carries it. `add_mint`
    /// assigns every column, so persisting a snapshot read out of a database
    /// with no mint row would erase what is stored.
    ///
    /// Writes are best effort: a wallet stays usable on its in-memory snapshot
    /// when the database rejects them, so each failure is logged rather than
    /// returned. Recording an incomplete write as done is not tolerable though,
    /// so the claim is given back unless every write succeeded, including when
    /// the caller is dropped between two of them. A rejection also puts the
    /// database on a growing backoff, because the claim coming back otherwise
    /// has every later `load` retry the write.
    ///
    /// # Arguments
    ///
    /// * `mint_url` - Mint URL for database keys
    /// * `storage` - Database to write to
    /// * `metadata` - Metadata to persist
    /// * `db_sync_versions` - Shared version tracker
    async fn persist_to_database(
        mint_url: MintUrl,
        storage: Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
        metadata: Arc<MintMetadata>,
        db_sync_versions: Arc<RwLock<HashMap<usize, SyncRecord>>>,
    ) {
        if !metadata.status.is_populated {
            return;
        }

        let Some(claim) = SyncClaim::take(db_sync_versions, &storage, metadata.versions()) else {
            return;
        };

        let mut persisted = true;

        // Save mint info
        if let Some(mint_info) = &metadata.mint_info {
            persisted &= storage
                .add_mint(mint_url.clone(), Some(mint_info.clone()))
                .await
                .inspect_err(|e| tracing::warn!("Failed to save mint info for {}: {}", mint_url, e))
                .is_ok();
        }

        // Save all keysets
        let keysets: Vec<_> = metadata.keysets.values().map(|ks| (**ks).clone()).collect();

        if !keysets.is_empty() {
            persisted &= storage
                .add_mint_keysets(mint_url.clone(), keysets)
                .await
                .inspect_err(|e| tracing::warn!("Failed to save keysets for {}: {}", mint_url, e))
                .is_ok();
        }

        // Save keys for each keyset
        for (keyset_id, keys) in &metadata.keys {
            if let Some(keyset_info) = metadata.keysets.get(keyset_id) {
                // Check if keys already exist in database to avoid duplicate insertion
                match storage.get_keys(keyset_id).await {
                    Ok(Some(_)) => {
                        tracing::trace!(
                            "Keys for keyset {} already in database, skipping insert",
                            keyset_id
                        );
                        continue;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!(
                            "Failed to read back keys for keyset {} at {}: {}",
                            keyset_id,
                            mint_url,
                            e
                        );
                        persisted = false;
                        continue;
                    }
                }

                let keyset = KeySet {
                    id: *keyset_id,
                    unit: keyset_info.unit.clone(),
                    active: Some(keyset_info.active),
                    input_fee_ppk: keyset_info.input_fee_ppk,
                    final_expiry: keyset_info.final_expiry,
                    keys: (**keys).clone(),
                };

                persisted &= storage
                    .add_keys(keyset)
                    .await
                    .inspect_err(|e| {
                        tracing::warn!(
                            "Failed to save keys for keyset {} at {}: {}",
                            keyset_id,
                            mint_url,
                            e
                        )
                    })
                    .is_ok();
            }
        }

        if persisted {
            claim.keep();
        } else {
            claim.reject();
        }
    }

    /// Fetch fresh metadata from mint HTTP API and update cache
    ///
    /// Performs the following steps:
    /// 1. Fetches mint info from server
    /// 2. Fetches list of all keysets
    /// 3. Fetches cryptographic keys for each keyset
    /// 4. Verifies keyset IDs match their keys
    /// 5. Atomically updates in-memory cache
    ///
    /// # Arguments
    ///
    /// * `client` - Optional regular mint client (for non-auth operations)
    /// * `auth_client` - Optional auth client (for blind auth keysets)
    ///
    /// # Returns
    ///
    /// Newly fetched and cached metadata
    ///
    /// Fetches everything first, then merges once, so a writer that finished
    /// while this one was in flight is not discarded. Keysets overwrite,
    /// because a `final_expiry` can change without the id changing; keys are
    /// immutable for an id, so whatever is already cached stands.
    async fn fetch_from_http(
        &self,
        client: Option<&Arc<dyn MintConnector + Send + Sync>>,
        auth_client: Option<&Arc<dyn AuthMintConnector + Send + Sync>>,
    ) -> Result<Arc<MintMetadata>, Error> {
        tracing::debug!(
            "Fetching mint metadata from HTTP for {}",
            escape_log_value(&self.mint_url)
        );

        let mut mint_info = None;
        let mut fetched_keysets = Vec::new();

        // Fetch regular mint data
        if let Some(client) = client.as_ref() {
            // Get mint information
            mint_info = Some(client.get_mint_info().await.inspect_err(|err| {
                tracing::error!(
                    "Failed to fetch mint info for {}: {}",
                    escape_log_value(&self.mint_url),
                    escape_log_value(err)
                );
            })?);

            // Get list of keysets
            fetched_keysets.extend(
                client
                    .get_mint_keysets()
                    .await
                    .inspect_err(|err| {
                        tracing::error!(
                            "Failed to fetch keysets for {}: {}",
                            escape_log_value(&self.mint_url),
                            escape_log_value(err)
                        );
                    })?
                    .keysets,
            );
        }

        // Fetch auth keysets if auth client provided
        if let Some(auth_client) = auth_client.as_ref() {
            fetched_keysets.extend(auth_client.get_mint_blind_auth_keysets().await?.keysets);
        }

        tracing::debug!(
            "Fetched {} keysets for {}",
            fetched_keysets.len(),
            escape_log_value(&self.mint_url)
        );

        let cached_keys = self.metadata.load().keys.clone();
        let mut fetched_keys = HashMap::new();
        for keyset_info in &fetched_keysets {
            if cached_keys.contains_key(&keyset_info.id) {
                continue;
            }

            let keyset = if keyset_info.unit == CurrencyUnit::Auth {
                auth_client
                    .as_ref()
                    .ok_or(Error::Internal)?
                    .get_mint_blind_auth_keyset(keyset_info.id)
                    .await?
            } else {
                client
                    .as_ref()
                    .ok_or(Error::Internal)?
                    .get_mint_keyset(keyset_info.id)
                    .await?
            };

            // Verify the keyset ID matches the keys
            keyset.verify_id()?;

            fetched_keys.insert(keyset_info.id, Arc::new(keyset.keys));
        }

        self.metadata.rcu(|current| {
            let mut next = (**current).clone();

            if let Some(mint_info) = mint_info.clone() {
                next.mint_info = Some(mint_info);
            }

            for keyset_info in &fetched_keysets {
                next.keysets
                    .insert(keyset_info.id, Arc::new(keyset_info.clone()));
            }

            for (id, keys) in &fetched_keys {
                next.keys.entry(*id).or_insert_with(|| keys.clone());
            }

            if client.is_some() {
                next.status.is_populated = true;
                next.status.updated_at = Instant::now();
                next.status.version += 1;
            }

            if auth_client.is_some() {
                next.auth_status.is_populated = true;
                next.auth_status.updated_at = Instant::now();
                next.auth_status.version += 1;
            }

            Arc::new(next)
        });

        let metadata = self.metadata.load().clone();

        tracing::info!(
            "Updated cache for {} with {} keysets (version {})",
            self.mint_url,
            metadata.keysets.len(),
            metadata.status.version
        );

        Ok(metadata)
    }

    /// Get the mint URL this cache manages
    pub fn mint_url(&self) -> &MintUrl {
        &self.mint_url
    }
}

#[cfg(test)]
mod tests {
    use cdk_common::nuts::KeySetInfo;

    use super::*;
    use crate::wallet::test_utils::{
        create_test_db, test_auth_keyset, test_keyset, test_keyset_id, test_mint_info,
        test_mint_url, AuthKeysetGate, GatedWalletDatabase, MockAuthMintConnector,
        MockMintConnector,
    };

    const OLD_EXPIRY: Option<u64> = Some(1_700_000_000);
    const NEW_EXPIRY: Option<u64> = Some(1_900_000_000);

    type Storage = Arc<dyn WalletDatabase<database::Error> + Send + Sync>;

    fn info(keyset: &KeySet, final_expiry: Option<u64>) -> KeySetInfo {
        KeySetInfo {
            id: keyset.id,
            unit: keyset.unit.clone(),
            active: keyset.active.unwrap_or(true),
            input_fee_ppk: keyset.input_fee_ppk,
            final_expiry,
        }
    }

    async fn storage_with(keysets: Vec<KeySetInfo>) -> Storage {
        let db = create_test_db().await;
        db.add_mint(test_mint_url(), None).await.unwrap();
        if !keysets.is_empty() {
            db.add_mint_keysets(test_mint_url(), keysets).await.unwrap();
        }
        db
    }

    fn unpersisted_snapshot(keyset: &KeySet, version: usize) -> Arc<MintMetadata> {
        let mut snapshot = MintMetadata {
            keysets: [(keyset.id, Arc::new(info(keyset, OLD_EXPIRY)))]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        snapshot.status.is_populated = true;
        snapshot.status.version = version;

        Arc::new(snapshot)
    }

    /// Brings a database's backoff forward to now, which stands in for waiting
    /// it out, and returns the wait it was serving.
    fn expire_backoff(cache: &MintMetadataCache, storage: &Storage) -> Duration {
        let mut records = cache.db_sync_versions.write();
        let backoff = records
            .get_mut(&MintMetadataCache::arc_pointer_id(storage))
            .and_then(|record| record.backoff.as_mut())
            .expect("a rejected write should have earned a backoff");

        backoff.next_attempt = Instant::now();

        backoff.delay
    }

    async fn stored_keysets(storage: &Storage) -> Vec<KeySetInfo> {
        storage
            .get_mint_keysets(test_mint_url())
            .await
            .unwrap()
            .unwrap_or_default()
    }

    fn mock_auth_client() -> Arc<dyn AuthMintConnector + Send + Sync> {
        Arc::new(MockAuthMintConnector::new())
    }

    fn mock_client(final_expiry: Option<u64>) -> Arc<dyn MintConnector + Send + Sync> {
        let client = MockMintConnector::new();
        let mut keyset = test_keyset();
        keyset.final_expiry = final_expiry;
        client.set_active_keyset(keyset);
        Arc::new(client)
    }

    #[tokio::test]
    async fn a_database_read_is_not_a_new_version() {
        let keyset = test_keyset();
        let storage = storage_with(vec![info(&keyset, OLD_EXPIRY)]).await;
        let cache = MintMetadataCache::new(test_mint_url());
        let before = cache.metadata.load().status.version;

        cache.load_from_db(&storage).await.unwrap();
        cache.load_from_db(&storage).await.unwrap();

        let metadata = cache.metadata.load();
        assert!(
            metadata.status.is_populated,
            "the keyset should have loaded"
        );
        assert_eq!(
            metadata.status.version, before,
            "reading a database is not a new version of the snapshot"
        );
        assert_eq!(
            cache
                .db_sync_versions
                .read()
                .get(&MintMetadataCache::arc_pointer_id(&storage))
                .and_then(|record| record.versions),
            Some(SnapshotVersions::default()),
            "a database that was only read from is recorded at version zero, so a later fetch still reaches it"
        );
    }

    #[tokio::test]
    async fn a_fetch_reaches_the_database_after_a_read_found_nothing() {
        let storage = storage_with(vec![]).await;
        let client = mock_client(NEW_EXPIRY);
        let cache = MintMetadataCache::new(test_mint_url());

        cache.load_from_db(&storage).await.unwrap();
        cache.load_from_mint(&storage, &client).await.unwrap();

        let stored = storage
            .get_mint_keysets(test_mint_url())
            .await
            .unwrap()
            .unwrap_or_default();
        assert_eq!(
            stored.len(),
            1,
            "the fetched keyset should have reached the database"
        );
        assert_eq!(stored[0].final_expiry, NEW_EXPIRY);
        assert!(
            storage.get_keys(&stored[0].id).await.unwrap().is_some(),
            "the fetched keys should have reached the database"
        );
    }

    #[tokio::test]
    async fn a_database_read_does_not_overwrite_fetched_keysets() {
        let keyset = test_keyset();
        let storage = storage_with(vec![info(&keyset, OLD_EXPIRY)]).await;
        let client = mock_client(NEW_EXPIRY);
        let cache = MintMetadataCache::new(test_mint_url());

        let fetched = cache.load_from_mint(&storage, &client).await.unwrap();
        assert_eq!(
            fetched.keysets.get(&keyset.id).unwrap().final_expiry,
            NEW_EXPIRY,
            "the mint reported a new expiry on an unchanged keyset id"
        );

        cache.load_from_db(&storage).await.unwrap();

        assert_eq!(
            cache
                .metadata
                .load()
                .keysets
                .get(&keyset.id)
                .unwrap()
                .final_expiry,
            NEW_EXPIRY,
            "the mint is authoritative, so a later database read must not put the old expiry back"
        );
    }

    #[tokio::test]
    async fn a_keyset_that_went_inactive_leaves_the_active_list() {
        let storage = storage_with(vec![]).await;
        let client = mock_client(OLD_EXPIRY);
        let cache = MintMetadataCache::new(test_mint_url());

        let fetched = cache.load_from_mint(&storage, &client).await.unwrap();
        assert_eq!(fetched.active_keysets().len(), 1);

        let mut retired = test_keyset();
        retired.final_expiry = OLD_EXPIRY;
        retired.active = Some(false);
        let retiring = MockMintConnector::new();
        retiring.set_active_keyset(retired);
        let retiring: Arc<dyn MintConnector + Send + Sync> = Arc::new(retiring);

        let refetched = cache.load_from_mint(&storage, &retiring).await.unwrap();

        assert!(
            refetched.active_keysets().is_empty(),
            "active keysets are derived from the merged keysets, so a retired one drops out"
        );
    }

    #[tokio::test]
    async fn an_auth_fetch_does_not_satisfy_a_forced_refresh() {
        let storage = storage_with(vec![]).await;
        let cache = MintMetadataCache::new(test_mint_url());

        cache
            .load_from_mint(&storage, &mock_client(OLD_EXPIRY))
            .await
            .unwrap();

        let auth_client = mock_auth_client();
        let fresh_client = mock_client(NEW_EXPIRY);

        // Stand in for a fetch already in flight, so the forced refresh below
        // samples the version and then waits for the lock.
        let in_flight = cache.fetch_lock.lock().await;

        let refresh = cache.load_from_mint(&storage, &fresh_client);
        let auth_then_release = async {
            cache
                .fetch_from_http(None, Some(&auth_client))
                .await
                .unwrap();
            drop(in_flight);
        };

        let (refreshed, ()) = tokio::join!(refresh, auth_then_release);

        assert_eq!(
            refreshed
                .unwrap()
                .keysets
                .get(&test_keyset_id())
                .unwrap()
                .final_expiry,
            NEW_EXPIRY,
            "an auth fetch is not a fetch of the regular keysets, so a forced \
             refresh waiting behind it must still reach the mint"
        );
    }

    #[tokio::test]
    async fn an_auth_fetch_keeps_what_a_regular_fetch_stored_while_it_waited() {
        let storage = storage_with(vec![]).await;
        let cache = MintMetadataCache::new(test_mint_url());
        let gate = AuthKeysetGate::default();
        let mut parked = MockAuthMintConnector::new();
        let auth_id = parked.keyset.id;
        parked.gate = Some(gate.clone());
        let auth_client: Arc<dyn AuthMintConnector + Send + Sync> = Arc::new(parked);

        let auth_fetch = cache.fetch_from_http(None, Some(&auth_client));
        let regular_fetch = async {
            gate.entered.notified().await;
            cache
                .load_from_mint(&storage, &mock_client(NEW_EXPIRY))
                .await
                .unwrap();
            gate.release.notify_one();
        };

        let (fetched, ()) = tokio::join!(auth_fetch, regular_fetch);
        let fetched = fetched.unwrap();

        assert!(
            fetched.keysets.contains_key(&auth_id),
            "the auth fetch should have reached the snapshot"
        );
        assert_eq!(
            fetched
                .keysets
                .get(&test_keyset_id())
                .map(|keyset| keyset.final_expiry),
            Some(NEW_EXPIRY),
            "the merge takes the snapshot as it stands when it runs, so a regular \
             fetch that finished while the auth fetch waited must survive it"
        );
        assert!(
            fetched.keys.contains_key(&test_keyset_id()),
            "its keys must survive too"
        );
        assert!(
            fetched.mint_info.is_some(),
            "and so must the mint info it brought"
        );
    }

    #[tokio::test]
    async fn an_auth_only_fetch_waits_for_regular_data() {
        let storage = storage_with(vec![]).await;
        let cache = MintMetadataCache::new(test_mint_url());
        let auth_id = test_auth_keyset().id;

        cache
            .load_auth(&storage, &mock_auth_client())
            .await
            .unwrap();

        let stored = storage
            .get_mint_keysets(test_mint_url())
            .await
            .unwrap()
            .unwrap_or_default();
        assert!(
            !stored.iter().any(|keyset| keyset.id == auth_id),
            "an auth-only snapshot leaves the regular status unpopulated, so a \
             restart would read these rows back as regular mint data. The mint row \
             is seeded so a stray write lands here instead of failing the keyset \
             foreign key"
        );
        assert!(
            storage.get_keys(&auth_id).await.unwrap().is_none(),
            "the auth keys wait with their keyset"
        );

        cache
            .load_from_mint(&storage, &mock_client(OLD_EXPIRY))
            .await
            .unwrap();

        let stored = storage
            .get_mint_keysets(test_mint_url())
            .await
            .unwrap()
            .unwrap_or_default();
        assert!(
            stored.iter().any(|keyset| keyset.id == auth_id),
            "the first regular persist carries the auth keyset with it"
        );
        assert!(
            storage.get_keys(&auth_id).await.unwrap().is_some(),
            "the auth keys should have reached the database with it"
        );
    }

    #[tokio::test]
    async fn an_auth_only_fetch_leaves_a_database_without_a_mint_row_usable() {
        let storage = create_test_db().await;
        let cache = MintMetadataCache::new(test_mint_url());
        let auth_id = test_auth_keyset().id;

        cache
            .load_auth(&storage, &mock_auth_client())
            .await
            .unwrap();

        assert!(
            storage.get_mint(test_mint_url()).await.unwrap().is_none(),
            "an auth-only snapshot carries no mint info, so nothing creates the row \
             its keysets need and the write has to wait"
        );

        cache
            .load_from_mint(&storage, &mock_client(OLD_EXPIRY))
            .await
            .unwrap();

        let stored = storage
            .get_mint_keysets(test_mint_url())
            .await
            .unwrap()
            .unwrap_or_default();
        assert!(
            stored.iter().any(|keyset| keyset.id == auth_id),
            "the regular fetch writes the mint row first, so the auth keyset reaches \
             a database that started without one"
        );
        assert!(
            stored.iter().any(|keyset| keyset.id == test_keyset_id()),
            "the regular keyset reaches it too"
        );
        assert!(
            storage.get_keys(&auth_id).await.unwrap().is_some(),
            "the auth keys come with their keyset"
        );
    }

    #[tokio::test]
    async fn a_database_holding_only_auth_keysets_is_not_populated() {
        let auth_keyset = test_auth_keyset();
        let storage = storage_with(vec![info(&auth_keyset, None)]).await;
        let cache = MintMetadataCache::new(test_mint_url());

        let loaded = cache.load_from_db(&storage).await.unwrap();

        assert!(
            !loaded.status.is_populated,
            "blind auth keysets are not regular mint data"
        );
        assert!(
            loaded.keysets.contains_key(&auth_keyset.id),
            "the auth keyset stays cached, so a later auth fetch skips its keys"
        );

        let metadata = cache
            .load(&storage, &mock_client(NEW_EXPIRY))
            .await
            .unwrap();

        assert_eq!(
            metadata
                .keysets
                .get(&test_keyset_id())
                .map(|keyset| keyset.final_expiry),
            Some(NEW_EXPIRY),
            "a database with no regular keyset must not keep the wallet off the mint"
        );
    }

    #[tokio::test]
    async fn a_database_read_reaches_a_second_database() {
        let keyset = test_keyset();
        let source = create_test_db().await;
        source
            .add_mint(test_mint_url(), Some(test_mint_info()))
            .await
            .unwrap();
        source
            .add_mint_keysets(test_mint_url(), vec![info(&keyset, OLD_EXPIRY)])
            .await
            .unwrap();
        source.add_keys(keyset.clone()).await.unwrap();

        let target = create_test_db().await;
        let cache = MintMetadataCache::new(test_mint_url());

        cache.load_from_db(&source).await.unwrap();
        cache.load(&target, &mock_client(NEW_EXPIRY)).await.unwrap();

        let stored = target
            .get_mint_keysets(test_mint_url())
            .await
            .unwrap()
            .unwrap_or_default();
        assert_eq!(
            stored.len(),
            1,
            "a snapshot read out of one database must still reach another"
        );
        assert_eq!(
            stored[0].final_expiry, OLD_EXPIRY,
            "the keyset came from the first database, so no fetch was needed"
        );
        assert!(
            target.get_keys(&keyset.id).await.unwrap().is_some(),
            "the keys should have reached the second database too"
        );
        assert_eq!(
            target
                .get_mint(test_mint_url())
                .await
                .unwrap()
                .and_then(|stored| stored.name),
            test_mint_info().name,
            "mint info the first database supplied should reach the second"
        );
    }

    #[tokio::test]
    async fn a_database_read_is_not_written_back_to_its_source() {
        let keyset = test_keyset();
        let source = storage_with(vec![info(&keyset, OLD_EXPIRY)]).await;
        source.add_keys(keyset.clone()).await.unwrap();
        let cache = MintMetadataCache::new(test_mint_url());
        let client = mock_client(OLD_EXPIRY);

        cache.load_from_db(&source).await.unwrap();

        let mut retired = info(&keyset, OLD_EXPIRY);
        retired.active = false;
        source
            .add_mint_keysets(test_mint_url(), vec![retired])
            .await
            .unwrap();
        cache.load(&source, &client).await.unwrap();

        let stored = source
            .get_mint_keysets(test_mint_url())
            .await
            .unwrap()
            .unwrap_or_default();
        assert!(
            !stored[0].active,
            "a snapshot read out of a database must not be written back to it"
        );
    }

    #[tokio::test]
    async fn a_persisted_snapshot_is_not_written_twice() {
        let keyset = test_keyset();
        let source = storage_with(vec![info(&keyset, OLD_EXPIRY)]).await;
        let target = create_test_db().await;
        let cache = MintMetadataCache::new(test_mint_url());
        let client = mock_client(OLD_EXPIRY);

        cache.load_from_db(&source).await.unwrap();
        cache.load(&target, &client).await.unwrap();

        let mut retired = info(&keyset, OLD_EXPIRY);
        retired.active = false;
        target
            .add_mint_keysets(test_mint_url(), vec![retired])
            .await
            .unwrap();
        cache.load(&target, &client).await.unwrap();

        let stored = target
            .get_mint_keysets(test_mint_url())
            .await
            .unwrap()
            .unwrap_or_default();
        assert!(
            !stored[0].active,
            "the snapshot was already persisted, so a second load must not write it again"
        );
    }

    #[tokio::test]
    async fn a_dropped_database_leaves_no_sync_record() {
        let first = storage_with(vec![]).await;
        let first_id = MintMetadataCache::arc_pointer_id(&first);
        let cache = MintMetadataCache::new(test_mint_url());

        cache
            .load_from_mint(&first, &mock_client(OLD_EXPIRY))
            .await
            .unwrap();
        assert!(
            cache.db_sync_versions.read().contains_key(&first_id),
            "the fetch should have been recorded against the first database"
        );

        drop(first);

        let second = storage_with(vec![]).await;
        cache.load(&second, &mock_client(OLD_EXPIRY)).await.unwrap();

        assert!(
            !cache.db_sync_versions.read().contains_key(&first_id),
            "a dropped database must not leave a record a recycled address could inherit"
        );
    }

    #[tokio::test]
    async fn a_snapshot_without_mint_info_leaves_stored_mint_info_alone() {
        let keyset = test_keyset();
        let storage = create_test_db().await;
        storage
            .add_mint(test_mint_url(), Some(test_mint_info()))
            .await
            .unwrap();
        let cache = MintMetadataCache::new(test_mint_url());

        cache
            .database_sync(storage.clone(), unpersisted_snapshot(&keyset, 1))
            .await;

        assert_eq!(
            storage
                .get_mint(test_mint_url())
                .await
                .unwrap()
                .and_then(|info| info.name),
            test_mint_info().name,
            "a snapshot read out of a database with no mint row carries no mint info, so \
             persisting it must not erase what is stored"
        );
    }

    #[tokio::test]
    async fn a_rejected_write_gives_its_claim_back() {
        let keyset = test_keyset();
        let storage = create_test_db().await;
        let cache = MintMetadataCache::new(test_mint_url());

        let snapshot = unpersisted_snapshot(&keyset, 1);

        cache.database_sync(storage.clone(), snapshot.clone()).await;

        assert!(
            storage
                .get_mint_keysets(test_mint_url())
                .await
                .unwrap()
                .unwrap_or_default()
                .is_empty(),
            "the snapshot carries no mint info, so nothing creates the mint row and the \
             keyset foreign key rejects the write"
        );

        storage.add_mint(test_mint_url(), None).await.unwrap();
        expire_backoff(&cache, &storage);

        cache.database_sync(storage.clone(), snapshot).await;

        assert_eq!(
            stored_keysets(&storage).await.len(),
            1,
            "a rejected write must not be recorded as done, so the same snapshot is \
             written again once the database can take it"
        );
    }

    #[tokio::test]
    async fn a_rejected_write_waits_out_its_backoff_before_being_retried() {
        let keyset = test_keyset();
        let storage = create_test_db().await;
        let cache = MintMetadataCache::new(test_mint_url());
        let snapshot = unpersisted_snapshot(&keyset, 1);

        cache.database_sync(storage.clone(), snapshot.clone()).await;

        storage.add_mint(test_mint_url(), None).await.unwrap();
        cache.database_sync(storage.clone(), snapshot.clone()).await;

        assert!(
            stored_keysets(&storage).await.is_empty(),
            "the database has just rejected this snapshot, so the next persist of it \
             has to wait rather than run on every load"
        );

        expire_backoff(&cache, &storage);
        cache.database_sync(storage.clone(), snapshot).await;

        assert_eq!(
            stored_keysets(&storage).await.len(),
            1,
            "the retry runs once the backoff has run out"
        );
    }

    #[tokio::test]
    async fn a_second_rejection_doubles_the_backoff() {
        let keyset = test_keyset();
        let storage = create_test_db().await;
        let cache = MintMetadataCache::new(test_mint_url());
        let snapshot = unpersisted_snapshot(&keyset, 1);

        cache.database_sync(storage.clone(), snapshot.clone()).await;
        let first = expire_backoff(&cache, &storage);

        cache.database_sync(storage.clone(), snapshot).await;
        let second = expire_backoff(&cache, &storage);

        assert_eq!(
            second,
            first * 2,
            "a database that keeps rejecting writes is left alone for longer each time"
        );
    }

    #[tokio::test]
    async fn a_database_that_took_the_write_is_not_held_off() {
        let keyset = test_keyset();
        let storage = create_test_db().await;
        let storage_id = MintMetadataCache::arc_pointer_id(&storage);
        let cache = MintMetadataCache::new(test_mint_url());
        let snapshot = unpersisted_snapshot(&keyset, 1);

        cache.database_sync(storage.clone(), snapshot.clone()).await;

        storage.add_mint(test_mint_url(), None).await.unwrap();
        expire_backoff(&cache, &storage);
        cache.database_sync(storage.clone(), snapshot).await;

        assert!(
            cache
                .db_sync_versions
                .read()
                .get(&storage_id)
                .is_some_and(|record| record.backoff.is_none()),
            "the backoff a rejection earned is spent once a persist succeeds"
        );
    }

    #[tokio::test]
    async fn a_success_leaves_the_backoff_of_a_newer_snapshot_alone() {
        let storage = create_test_db().await;
        let cache = MintMetadataCache::new(test_mint_url());
        let older = SnapshotVersions { status: 1, auth: 0 };
        let newer = SnapshotVersions { status: 2, auth: 0 };

        let first = SyncClaim::take(cache.db_sync_versions.clone(), &storage, older)
            .expect("an untouched database gives up the first claim");

        SyncClaim::take(cache.db_sync_versions.clone(), &storage, newer)
            .expect("a newer snapshot claims over one still in flight")
            .reject();

        first.keep();

        assert!(
            cache
                .db_sync_versions
                .read()
                .get(&MintMetadataCache::arc_pointer_id(&storage))
                .is_some_and(|record| record.backoff.is_some()),
            "an older persist succeeding says nothing about the newer snapshot the \
             database refused"
        );
    }

    #[tokio::test]
    async fn a_backoff_does_not_hold_back_a_newer_snapshot() {
        let keyset = test_keyset();
        let storage = create_test_db().await;
        let cache = MintMetadataCache::new(test_mint_url());

        cache
            .database_sync(storage.clone(), unpersisted_snapshot(&keyset, 1))
            .await;

        storage.add_mint(test_mint_url(), None).await.unwrap();
        cache
            .database_sync(storage.clone(), unpersisted_snapshot(&keyset, 2))
            .await;

        assert_eq!(
            stored_keysets(&storage).await.len(),
            1,
            "the backoff bounds retries of the snapshot that failed, not a fetch that \
             has since produced a newer one"
        );
    }

    #[tokio::test]
    async fn a_cancelled_persist_gives_its_claim_back() {
        let keyset = test_keyset();
        let database = create_test_db().await;
        database.add_mint(test_mint_url(), None).await.unwrap();
        let (gated, suspended) = GatedWalletDatabase::new(database.clone());
        let storage: Storage = gated;
        let storage_id = MintMetadataCache::arc_pointer_id(&storage);
        let cache = MintMetadataCache::new(test_mint_url());

        let snapshot = unpersisted_snapshot(&keyset, 1);

        let mut persist = Box::pin(cache.database_sync(storage.clone(), snapshot.clone()));

        tokio::select! {
            _ = &mut persist => panic!("the gated keyset write should not have finished"),
            _ = suspended.notified() => {}
        }

        assert_eq!(
            cache
                .db_sync_versions
                .read()
                .get(&storage_id)
                .and_then(|record| record.versions),
            Some(snapshot.versions()),
            "the version is claimed before the writes, so the suspended persist holds it"
        );

        drop(persist);

        assert_eq!(
            cache
                .db_sync_versions
                .read()
                .get(&storage_id)
                .and_then(|record| record.versions),
            None,
            "a caller dropped mid-write leaves nothing persisted, so the claim has to go \
             back with it"
        );

        cache.database_sync(storage.clone(), snapshot).await;

        let stored = database
            .get_mint_keysets(test_mint_url())
            .await
            .unwrap()
            .unwrap_or_default();
        assert_eq!(
            stored.len(),
            1,
            "a cancelled persist is not a database that rejected anything, so the next \
             persist of the same snapshot retries the writes it never made"
        );
    }
}

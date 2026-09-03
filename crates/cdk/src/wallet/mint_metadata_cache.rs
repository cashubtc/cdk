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

use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::sync::Arc;
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

/// Maximum number of unreferenced keyset descriptions retained in persistence.
///
/// HTTP response bodies are already bounded by the transport. This separate,
/// larger bound prevents a mint that rotates otherwise-valid snapshots from
/// growing an append-only wallet database indefinitely.
const MAX_PERSISTED_KEYSET_DESCRIPTIONS: usize = 1_000;

/// Maximum number of per-ID keyset requests made during one metadata refresh.
///
/// Cached keys do not count towards this limit.
const MAX_UNCACHED_KEYSET_FETCHES_PER_REFRESH: usize = 32;

fn keyset_ids_in_use(
    proofs: impl IntoIterator<Item = cdk_common::wallet::ProofInfo>,
) -> HashSet<Id> {
    proofs
        .into_iter()
        .map(|proof| proof.proof.keyset_id)
        .collect()
}

fn prioritize_keysets(keysets: &mut [KeySetInfo]) {
    // A stable sort preserves the mint's order within each class, while ensuring
    // inactive history cannot consume the fetch budget before active keysets.
    keysets.sort_by_key(|keyset| !keyset.active);
}

fn prioritize_refresh_keysets(keysets: &mut [KeySetInfo], protected: &HashSet<Id>) {
    // Proof-bearing keysets are as important as active keysets: both must be
    // fetched before optional history regardless of response ordering.
    keysets.sort_by_key(|keyset| !(keyset.active || protected.contains(&keyset.id)));
}

fn required_uncached_keysets(
    keysets: &[KeySetInfo],
    keys: &HashMap<Id, Arc<Keys>>,
    protected: &HashSet<Id>,
) -> usize {
    keysets
        .iter()
        .filter(|keyset| {
            (keyset.active || protected.contains(&keyset.id)) && !keys.contains_key(&keyset.id)
        })
        .count()
}

fn retain_refreshed_domain(
    metadata: &mut MintMetadata,
    auth: bool,
    advertised: &HashSet<Id>,
    protected: &HashSet<Id>,
) {
    metadata.keysets.retain(|id, keyset| {
        let is_auth = keyset.unit == CurrencyUnit::Auth;
        is_auth != auth || advertised.contains(id) || protected.contains(id)
    });
    metadata
        .keys
        .retain(|id, _| metadata.keysets.contains_key(id));
}

fn select_persisted_keysets(
    mut keysets: Vec<KeySetInfo>,
    protected: &HashSet<Id>,
) -> Vec<KeySetInfo> {
    prioritize_keysets(&mut keysets);

    let mut regular_optional = 0;
    let mut auth_optional = 0;
    keysets
        .into_iter()
        .filter(|keyset| {
            if keyset.active || protected.contains(&keyset.id) {
                true
            } else {
                let optional = if keyset.unit == CurrencyUnit::Auth {
                    &mut auth_optional
                } else {
                    &mut regular_optional
                };
                if *optional < MAX_PERSISTED_KEYSET_DESCRIPTIONS {
                    *optional += 1;
                    true
                } else {
                    false
                }
            }
        })
        .collect()
}

fn select_keysets_to_persist(
    mut keysets: Vec<KeySetInfo>,
    existing: &HashSet<Id>,
    protected: &HashSet<Id>,
) -> Vec<KeySetInfo> {
    // Update existing rows and always save proof-bearing descriptions. Fill any
    // remaining slots with active descriptions before inactive history.
    keysets.sort_by_key(|keyset| {
        (
            !(existing.contains(&keyset.id) || protected.contains(&keyset.id)),
            !keyset.active,
        )
    });

    let mut persisted_ids = existing.clone();
    keysets
        .into_iter()
        .filter(|keyset| {
            let should_persist = persisted_ids.contains(&keyset.id)
                || protected.contains(&keyset.id)
                || persisted_ids.len() < MAX_PERSISTED_KEYSET_DESCRIPTIONS;
            if should_persist {
                persisted_ids.insert(keyset.id);
            }
            should_persist
        })
        .collect()
}

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

    /// Monotonically increasing generation for this freshness domain.
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

fn mark_refreshed(status: &mut FreshnessStatus, complete: bool) {
    if complete {
        status.is_populated = true;
        status.updated_at = Instant::now();
    }
}

fn record_successful_refresh(metadata: &mut MintMetadata, regular: bool, auth: bool) {
    if regular {
        mark_refreshed(&mut metadata.status, true);
        metadata.status.version += 1;
    }
    if auth {
        mark_refreshed(&mut metadata.auth_status, true);
        metadata.auth_status.version += 1;
    }
    metadata.persistence_version += 1;
}

fn should_persist_mint_info(metadata: &MintMetadata) -> bool {
    metadata.status.is_populated
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
    /// Mint server information (name, description, supported features, etc.)
    pub mint_info: MintInfo,

    /// All keysets indexed by their ID (includes both active and inactive)
    pub keysets: HashMap<Id, Arc<KeySetInfo>>,

    /// Cryptographic keys for each keyset, indexed by keyset ID
    pub keys: HashMap<Id, Arc<Keys>>,

    /// Subset of keysets that are currently active (cached for convenience)
    pub active_keysets: Vec<Arc<KeySetInfo>>,

    /// Freshness tracking for regular (non-auth) mint data
    pub(crate) status: FreshnessStatus,

    /// Freshness tracking for blind auth keysets
    auth_status: FreshnessStatus,

    /// Generation of any mutation that needs database persistence.
    persistence_version: usize,
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

    /// Tracks which database instances have been synced to which cache version.
    /// Key: pointer identity of storage Arc, Value: last synced cache version
    db_sync_versions: Arc<RwLock<HashMap<usize, usize>>>,

    /// Serializes persistence attempts so an older snapshot cannot finish after
    /// and overwrite a newer successfully persisted snapshot.
    db_sync_lock: Arc<Mutex<()>>,

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
            db_sync_lock: Arc::new(Mutex::new(())),
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
    /// Use this when you need guaranteed fresh data from the mint.
    ///
    /// # Arguments
    ///
    /// * `storage` - Database to persist metadata to (async background write)
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

        // Check if another caller already updated the cache while we waited
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
            let _ = self.load_from_db(storage).await;
        }

        let protected_keysets = match storage
            .get_proofs(Some(self.mint_url.clone()), None, None, None)
            .await
        {
            Ok(proofs) => keyset_ids_in_use(proofs),
            Err(err) => {
                tracing::warn!(
                    "Failed to inspect proof keysets for {}; preserving all cached keysets: {}",
                    self.mint_url,
                    err
                );
                self.metadata.load().keysets.keys().copied().collect()
            }
        };

        // Perform the fetch
        // Note: keys already in cache (e.g. from load_from_db at boot) are
        // skipped by fetch_from_http's Vacant entry check.
        let metadata = self
            .fetch_from_http(Some(client), None, &protected_keysets)
            .await?;

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
    /// Uses a mutex to ensure only one load runs at a time. If multiple
    /// callers request a load simultaneously, only one reads from the database
    /// while others wait for the lock, then return the updated cache.
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
        let mut new_metadata = (*self.metadata.load().clone()).clone();

        // Load mint info
        if let Some(mint_info) = storage.get_mint(self.mint_url.clone()).await? {
            new_metadata.mint_info = mint_info;
        }

        // Load keysets and their keys
        if let Some(keysets) = storage.get_mint_keysets(self.mint_url.clone()).await? {
            let protected_keysets = keyset_ids_in_use(
                storage
                    .get_proofs(Some(self.mint_url.clone()), None, None, None)
                    .await?,
            );
            let selected_keysets = select_persisted_keysets(keysets, &protected_keysets);
            let selected_ids = selected_keysets.iter().map(|keyset| keyset.id).collect();
            retain_refreshed_domain(&mut new_metadata, false, &selected_ids, &protected_keysets);
            retain_refreshed_domain(&mut new_metadata, true, &selected_ids, &protected_keysets);
            new_metadata.active_keysets.clear();
            for keyset_info in selected_keysets {
                let keyset_arc = Arc::new(keyset_info.clone());
                new_metadata
                    .keysets
                    .insert(keyset_info.id, keyset_arc.clone());

                if keyset_info.active {
                    new_metadata.active_keysets.push(keyset_arc);
                }

                if let Some(keys) = storage.get_keys(&keyset_info.id).await? {
                    tracing::trace!("Loaded keys for keyset {} from database", keyset_info.id);
                    new_metadata.keys.insert(keyset_info.id, Arc::new(keys));
                }
            }
        }

        // Only mark as populated if we actually loaded keysets.
        // Don't update `updated_at` — the TTL should reflect when we last
        // fetched from the mint, not when we read from the local DB.
        new_metadata.status.is_populated = new_metadata
            .keysets
            .values()
            .any(|keyset| keyset.unit != CurrencyUnit::Auth);
        new_metadata.auth_status.is_populated = new_metadata
            .keysets
            .values()
            .any(|keyset| keyset.unit == CurrencyUnit::Auth);
        new_metadata.status.version += 1;
        new_metadata.auth_status.version += 1;
        new_metadata.persistence_version += 1;

        tracing::info!(
            "Loaded cache from database for {} with {} keysets (version {})",
            self.mint_url,
            new_metadata.keysets.len(),
            new_metadata.status.version
        );

        // Atomically update cache
        let metadata_arc = Arc::new(new_metadata);
        self.metadata.store(metadata_arc.clone());

        // Mark DB as synced (data came from DB, no need to write back)
        let storage_id = Self::arc_pointer_id(storage);
        self.db_sync_versions
            .write()
            .insert(storage_id, metadata_arc.persistence_version);

        Ok(metadata_arc)
    }

    /// Load metadata from cache or fetch if not available
    ///
    /// Returns cached metadata if available and it is still valid, otherwise fetches from the mint.
    /// If cache is stale relative to the database, spawns a background sync task.
    ///
    /// This is the primary method for normal operations - it balances freshness
    /// with performance by returning cached data when available.
    ///
    /// # Arguments
    ///
    /// * `storage` - Database to persist metadata to (if fetched or stale)
    /// * `client` - HTTP client for fetching from mint (only if cache empty)
    /// * `ttl` - Optional TTL, if not provided it is assumed that any cached data is good enough
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
            .cloned()
            .unwrap_or_default();

        if cached_metadata.status.is_populated
            && ttl
                .map(|ttl| cached_metadata.status.updated_at + ttl > Instant::now())
                .unwrap_or(true)
        {
            // Cache is ready - check if database needs updating
            if db_synced_version != cached_metadata.persistence_version {
                // Database is stale - sync before returning
                self.database_sync(storage.clone(), cached_metadata.clone())
                    .await;
            }
            return Ok(cached_metadata);
        }

        // In-memory cache was empty (not just stale) — try database before network
        if !cached_metadata.status.is_populated {
            if let Ok(metadata) = self.load_from_db(storage).await {
                if metadata.status.is_populated {
                    return Ok(metadata);
                }
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
    /// * `ttl` - Optional TTL; if not provided, any populated cached data is
    ///   considered fresh and no HTTP fetch is performed
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
            .cloned()
            .unwrap_or_default();

        // Check if auth data is populated in cache and still fresh
        if cached_metadata.auth_status.is_populated
            && ttl
                .map(|ttl| cached_metadata.auth_status.updated_at + ttl > Instant::now())
                .unwrap_or(true)
        {
            if db_synced_version != cached_metadata.persistence_version {
                // Database needs updating - sync before returning
                self.database_sync(storage.clone(), cached_metadata.clone())
                    .await;
            }
            return Ok(cached_metadata);
        }

        // Acquire fetch lock to ensure only one auth fetch at a time
        let _guard = self.fetch_lock.lock().await;

        // Re-check if auth data was updated while waiting for lock
        let mut current_metadata = self.metadata.load().clone();
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

        // An auth-only refresh must start from the persisted regular snapshot.
        // Otherwise its default MintInfo could overwrite valid regular metadata.
        if !current_metadata.status.is_populated {
            match self.load_from_db(storage).await {
                Ok(metadata) => {
                    current_metadata = metadata;
                    if current_metadata.auth_status.is_populated
                        && ttl
                            .map(|ttl| {
                                current_metadata.auth_status.updated_at + ttl > Instant::now()
                            })
                            .unwrap_or(true)
                    {
                        return Ok(current_metadata);
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        "Failed to load regular metadata before auth refresh for {}: {}",
                        self.mint_url,
                        err
                    );
                }
            }
        }

        // Auth data not in cache - fetch from mint
        let protected_keysets = match storage
            .get_proofs(Some(self.mint_url.clone()), None, None, None)
            .await
        {
            Ok(proofs) => keyset_ids_in_use(proofs),
            Err(err) => {
                tracing::warn!(
                    "Failed to inspect proof keysets for {}; preserving all cached keysets: {}",
                    self.mint_url,
                    err
                );
                self.metadata.load().keysets.keys().copied().collect()
            }
        };
        let metadata = self
            .fetch_from_http(None, Some(auth_client), &protected_keysets)
            .await?;

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
        let _guard = self.db_sync_lock.lock().await;
        let mint_url = self.mint_url.clone();
        let db_sync_versions = self.db_sync_versions.clone();

        if let Err(err) =
            Self::persist_to_database(mint_url.clone(), storage, metadata, db_sync_versions).await
        {
            tracing::warn!("Failed to persist metadata for {}: {}", mint_url, err);
        }
    }

    /// Persist metadata to database
    ///
    /// Saves mint info, keysets, and keys to the database. Checks version
    /// before writing to avoid redundant work if a newer version has already
    /// been persisted.
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
        db_sync_versions: Arc<RwLock<HashMap<usize, usize>>>,
    ) -> Result<(), database::Error> {
        let storage_id = Self::arc_pointer_id(&storage);

        // Check if this write is still needed
        {
            let versions = db_sync_versions.read();
            let current_synced_version = versions.get(&storage_id).cloned().unwrap_or_default();

            if metadata.persistence_version <= current_synced_version {
                // A newer version has already been persisted - skip this write
                return Ok(());
            }
        }

        // Save mint info
        if should_persist_mint_info(&metadata) {
            storage
                .add_mint(mint_url.clone(), Some(metadata.mint_info.clone()))
                .await?;
        }

        // The database API can add or update descriptions but cannot remove
        // them. Refuse new, unreferenced IDs after the cumulative cap while
        // continuing to update existing rows and preserve proof keysets.
        let existing_keysets = storage
            .get_mint_keysets(mint_url.clone())
            .await?
            .unwrap_or_default();
        let protected_keysets = keyset_ids_in_use(
            storage
                .get_proofs(Some(mint_url.clone()), None, None, None)
                .await?,
        );
        let existing_ids = existing_keysets
            .into_iter()
            .map(|keyset| keyset.id)
            .collect();
        let keysets = select_keysets_to_persist(
            metadata
                .keysets
                .values()
                .map(|keyset| (**keyset).clone())
                .collect(),
            &existing_ids,
            &protected_keysets,
        );
        let persisted_keyset_ids: HashSet<_> = keysets.iter().map(|keyset| keyset.id).collect();

        if !keysets.is_empty() {
            storage.add_mint_keysets(mint_url.clone(), keysets).await?;
        }

        // Save keys for each keyset
        for (keyset_id, keys) in &metadata.keys {
            if !persisted_keyset_ids.contains(keyset_id) {
                continue;
            }
            if let Some(keyset_info) = metadata.keysets.get(keyset_id) {
                // Check if keys already exist in database to avoid duplicate insertion
                if storage.get_keys(keyset_id).await?.is_some() {
                    tracing::trace!(
                        "Keys for keyset {} already in database, skipping insert",
                        keyset_id
                    );
                    continue;
                }

                let keyset = KeySet {
                    id: *keyset_id,
                    unit: keyset_info.unit.clone(),
                    active: Some(keyset_info.active),
                    input_fee_ppk: keyset_info.input_fee_ppk,
                    final_expiry: keyset_info.final_expiry,
                    keys: (**keys).clone(),
                };

                storage.add_keys(keyset).await?;
            }
        }

        // Only a fully successful attempt is considered synced. Concurrent
        // older writes must not lower a newer completed generation.
        db_sync_versions
            .write()
            .entry(storage_id)
            .and_modify(|version| {
                *version = (*version).max(metadata.persistence_version);
            })
            .or_insert(metadata.persistence_version);
        Ok(())
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
    async fn fetch_from_http(
        &self,
        client: Option<&Arc<dyn MintConnector + Send + Sync>>,
        auth_client: Option<&Arc<dyn AuthMintConnector + Send + Sync>>,
        protected_keysets: &HashSet<Id>,
    ) -> Result<Arc<MintMetadata>, Error> {
        tracing::debug!(
            "Fetching mint metadata from HTTP for {}",
            escape_log_value(&self.mint_url)
        );

        // Start with current cache to preserve data from other sources
        let mut new_metadata = (*self.metadata.load().clone()).clone();
        let mut regular_keysets = Vec::new();
        let mut auth_keysets = Vec::new();

        // Fetch regular mint data
        if let Some(client) = client.as_ref() {
            // Get mint information
            new_metadata.mint_info = client.get_mint_info().await.inspect_err(|err| {
                tracing::error!(
                    "Failed to fetch mint info for {}: {}",
                    escape_log_value(&self.mint_url),
                    escape_log_value(err)
                );
            })?;

            // Get list of keysets
            regular_keysets = client
                .get_mint_keysets()
                .await
                .inspect_err(|err| {
                    tracing::error!(
                        "Failed to fetch keysets for {}: {}",
                        escape_log_value(&self.mint_url),
                        escape_log_value(err)
                    );
                })?
                .keysets;
        }

        // Fetch auth keysets if auth client provided
        if let Some(auth_client) = auth_client.as_ref() {
            auth_keysets = auth_client.get_mint_blind_auth_keysets().await?.keysets;
        }

        if regular_keysets
            .iter()
            .any(|keyset| keyset.unit == CurrencyUnit::Auth)
            || auth_keysets
                .iter()
                .any(|keyset| keyset.unit != CurrencyUnit::Auth)
        {
            return Err(Error::Custom(
                "Mint returned a keyset in the wrong metadata endpoint".to_string(),
            ));
        }

        prioritize_refresh_keysets(&mut regular_keysets, protected_keysets);
        prioritize_refresh_keysets(&mut auth_keysets, protected_keysets);

        tracing::debug!(
            "Fetched {} keysets for {}",
            regular_keysets.len() + auth_keysets.len(),
            escape_log_value(&self.mint_url)
        );

        if client.is_some() {
            let advertised = regular_keysets.iter().map(|keyset| keyset.id).collect();
            retain_refreshed_domain(&mut new_metadata, false, &advertised, protected_keysets);
        }
        if auth_client.is_some() {
            let advertised = auth_keysets.iter().map(|keyset| keyset.id).collect();
            retain_refreshed_domain(&mut new_metadata, true, &advertised, protected_keysets);
        }

        let required_regular =
            required_uncached_keysets(&regular_keysets, &new_metadata.keys, protected_keysets);
        let required_auth =
            required_uncached_keysets(&auth_keysets, &new_metadata.keys, protected_keysets);
        if required_regular > MAX_UNCACHED_KEYSET_FETCHES_PER_REFRESH
            || required_auth > MAX_UNCACHED_KEYSET_FETCHES_PER_REFRESH
        {
            tracing::warn!(
                mint_url = %self.mint_url,
                required_regular,
                required_auth,
                limit = MAX_UNCACHED_KEYSET_FETCHES_PER_REFRESH,
                "Required key material exceeds per-domain refresh limit; preserving previous cache"
            );
            return Err(Error::Custom(
                "Mint metadata requires too many uncached active or proof keysets".to_string(),
            ));
        }

        // Fetch keys for each keyset
        let mut regular_uncached_fetches = 0;
        let mut auth_uncached_fetches = 0;
        let mut skipped_regular_keys = 0;
        let mut skipped_auth_keys = 0;
        for keyset_info in regular_keysets.into_iter().chain(auth_keysets) {
            let is_auth = keyset_info.unit == CurrencyUnit::Auth;
            let keyset_arc = Arc::new(keyset_info.clone());
            new_metadata
                .keysets
                .insert(keyset_info.id, keyset_arc.clone());

            // Only fetch keys if we don't already have them cached
            let _has_keys = match new_metadata.keys.entry(keyset_info.id) {
                std::collections::hash_map::Entry::Occupied(_) => true,
                std::collections::hash_map::Entry::Vacant(e) => {
                    let fetches = if is_auth {
                        &mut auth_uncached_fetches
                    } else {
                        &mut regular_uncached_fetches
                    };
                    if *fetches >= MAX_UNCACHED_KEYSET_FETCHES_PER_REFRESH {
                        if is_auth {
                            skipped_auth_keys += 1;
                        } else {
                            skipped_regular_keys += 1;
                        }
                        false
                    } else {
                        *fetches += 1;

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

                        e.insert(Arc::new(keyset.keys));
                        true
                    }
                }
            };

            // Descriptions are useful independently of cached key material
            // (notably when resolving token proofs), so skipped optional keys do
            // not remove their successfully fetched descriptions.
        }

        if skipped_regular_keys > 0 || skipped_auth_keys > 0 {
            tracing::warn!(
                mint_url = %self.mint_url,
                skipped_regular = skipped_regular_keys,
                skipped_auth = skipped_auth_keys,
                limit = MAX_UNCACHED_KEYSET_FETCHES_PER_REFRESH,
                "Skipped optional inactive key material after reaching the fetch limit"
            );
        }

        new_metadata.active_keysets = new_metadata
            .keysets
            .values()
            .filter(|keyset| keyset.active && new_metadata.keys.contains_key(&keyset.id))
            .cloned()
            .collect();

        // Update freshness status based on what was fetched
        record_successful_refresh(&mut new_metadata, client.is_some(), auth_client.is_some());

        tracing::info!(
            "Updated cache for {} with {} keysets (version {})",
            self.mint_url,
            new_metadata.keysets.len(),
            new_metadata.status.version
        );

        // Atomically update cache
        let metadata_arc = Arc::new(new_metadata);
        self.metadata.store(metadata_arc.clone());
        Ok(metadata_arc)
    }

    /// Get the mint URL this cache manages
    pub fn mint_url(&self) -> &MintUrl {
        &self.mint_url
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(index: u64) -> Id {
        Id::from_bytes(&index.to_be_bytes()).expect("eight-byte keyset id")
    }

    fn keyset(index: u64, unit: CurrencyUnit, active: bool) -> KeySetInfo {
        KeySetInfo {
            id: id(index),
            unit,
            active,
            input_fee_ppk: 0,
            final_expiry: None,
        }
    }

    fn insert_keyset(metadata: &mut MintMetadata, keyset: KeySetInfo) {
        metadata.keysets.insert(keyset.id, Arc::new(keyset));
    }

    #[test]
    fn active_keysets_are_prioritized_beyond_inactive_fetch_limit() {
        let mut keysets: Vec<_> = (0..MAX_UNCACHED_KEYSET_FETCHES_PER_REFRESH as u64 + 10)
            .map(|index| keyset(index, CurrencyUnit::Sat, false))
            .collect();
        let active_id = id(1_000);
        keysets.push(keyset(1_000, CurrencyUnit::Sat, true));

        prioritize_refresh_keysets(&mut keysets, &HashSet::new());

        assert_eq!(keysets.first().map(|keyset| keyset.id), Some(active_id));
    }

    #[test]
    fn refreshing_auth_does_not_evict_or_compete_with_regular_keysets() {
        let mut metadata = MintMetadata::default();
        let regular_id = id(1);
        let old_auth_id = id(2);
        let current_auth_id = id(3);
        insert_keyset(&mut metadata, keyset(1, CurrencyUnit::Sat, true));
        insert_keyset(&mut metadata, keyset(2, CurrencyUnit::Auth, false));
        insert_keyset(&mut metadata, keyset(3, CurrencyUnit::Auth, true));

        retain_refreshed_domain(
            &mut metadata,
            true,
            &HashSet::from([current_auth_id]),
            &HashSet::new(),
        );

        assert!(metadata.keysets.contains_key(&regular_id));
        assert!(metadata.keysets.contains_key(&current_auth_id));
        assert!(!metadata.keysets.contains_key(&old_auth_id));
    }

    #[test]
    fn rotating_snapshots_do_not_grow_cache_and_proof_keysets_are_retained() {
        let mut metadata = MintMetadata::default();
        let proof_keyset_id = id(10_000);
        insert_keyset(&mut metadata, keyset(10_000, CurrencyUnit::Sat, false));
        let protected = HashSet::from([proof_keyset_id]);
        // This deliberately exceeds the former 100-description rejection limit.
        const SNAPSHOT_SIZE: u64 = 150;

        for generation in 0..20 {
            let first = generation * SNAPSHOT_SIZE;
            let advertised: HashSet<_> = (first..first + SNAPSHOT_SIZE).map(id).collect();
            retain_refreshed_domain(&mut metadata, false, &advertised, &protected);
            for index in first..first + SNAPSHOT_SIZE {
                insert_keyset(&mut metadata, keyset(index, CurrencyUnit::Sat, false));
            }

            assert!(metadata.keysets.contains_key(&proof_keyset_id));
            assert_eq!(metadata.keysets.len(), SNAPSHOT_SIZE as usize + 1);
        }
    }

    #[test]
    fn persisted_selection_keeps_active_and_proof_keysets_past_limit() {
        let proof_keyset_id = id(20_000);
        let active_keyset_id = id(20_001);
        let mut keysets: Vec<_> = (0..MAX_PERSISTED_KEYSET_DESCRIPTIONS as u64 + 20)
            .map(|index| keyset(index, CurrencyUnit::Sat, false))
            .collect();
        keysets.push(keyset(20_000, CurrencyUnit::Sat, false));
        keysets.push(keyset(20_001, CurrencyUnit::Sat, true));

        let selected = select_persisted_keysets(keysets, &HashSet::from([proof_keyset_id]));

        assert!(selected.iter().any(|keyset| keyset.id == proof_keyset_id));
        assert!(selected.iter().any(|keyset| keyset.id == active_keyset_id));
        assert_eq!(selected.len(), MAX_PERSISTED_KEYSET_DESCRIPTIONS + 2);
    }

    #[test]
    fn successful_description_refresh_is_fresh_without_optional_keys() {
        let mut status = FreshnessStatus::default();

        mark_refreshed(&mut status, true);

        assert!(status.is_populated);
    }

    #[test]
    fn skipped_optional_key_material_keeps_its_description() {
        let mut metadata = MintMetadata::default();
        let description = keyset(30_000, CurrencyUnit::Sat, false);
        insert_keyset(&mut metadata, description.clone());

        assert_eq!(
            required_uncached_keysets(
                std::slice::from_ref(&description),
                &metadata.keys,
                &HashSet::new(),
            ),
            0
        );
        assert!(metadata.keysets.contains_key(&description.id));
        assert!(!metadata.keys.contains_key(&description.id));
    }

    #[test]
    fn too_many_required_uncached_keys_exceeds_hard_fetch_cap() {
        let keysets: Vec<_> = (0..=MAX_UNCACHED_KEYSET_FETCHES_PER_REFRESH as u64)
            .map(|index| keyset(index, CurrencyUnit::Sat, true))
            .collect();

        assert!(
            required_uncached_keysets(&keysets, &HashMap::new(), &HashSet::new(),)
                > MAX_UNCACHED_KEYSET_FETCHES_PER_REFRESH
        );
    }

    #[test]
    fn auth_refresh_does_not_advance_regular_waiter_generation() {
        let mut metadata = MintMetadata::default();
        let regular_version = metadata.status.version;

        record_successful_refresh(&mut metadata, false, true);

        assert_eq!(metadata.status.version, regular_version);
        assert_eq!(metadata.auth_status.version, 1);
        assert_eq!(metadata.persistence_version, 1);
        assert!(!should_persist_mint_info(&metadata));
    }

    #[test]
    fn proof_key_material_counts_toward_hard_fetch_cap() {
        let keysets: Vec<_> = (0..=MAX_UNCACHED_KEYSET_FETCHES_PER_REFRESH as u64)
            .map(|index| keyset(index, CurrencyUnit::Sat, false))
            .collect();
        let protected = keysets.iter().map(|keyset| keyset.id).collect();

        assert!(
            required_uncached_keysets(&keysets, &HashMap::new(), &protected,)
                > MAX_UNCACHED_KEYSET_FETCHES_PER_REFRESH
        );
    }

    #[test]
    fn persistence_cap_updates_existing_and_allows_proof_keysets_only() {
        let existing: HashSet<_> = (0..MAX_PERSISTED_KEYSET_DESCRIPTIONS as u64)
            .map(id)
            .collect();
        let attacker_id = id(20_000);
        let proof_id = id(20_001);
        let selected = select_keysets_to_persist(
            vec![
                keyset(0, CurrencyUnit::Sat, false),
                keyset(20_000, CurrencyUnit::Sat, true),
                keyset(20_001, CurrencyUnit::Sat, false),
            ],
            &existing,
            &HashSet::from([proof_id]),
        );

        assert!(selected.iter().any(|keyset| keyset.id == id(0)));
        assert!(selected.iter().any(|keyset| keyset.id == proof_id));
        assert!(!selected.iter().any(|keyset| keyset.id == attacker_id));
    }
}

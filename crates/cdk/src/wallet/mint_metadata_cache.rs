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

use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use cdk_common::database::{self, WalletDatabase};
use cdk_common::mint_url::MintUrl;
use cdk_common::nuts::{KeySetInfo, Keys};
use cdk_common::parking_lot::RwLock;
use cdk_common::{KeySet, MintInfo};
use tokio::sync::Mutex;
use web_time::Instant;

use crate::nuts::Id;
use crate::wallet::MintConnector;
#[cfg(feature = "auth")]
use crate::wallet::{AuthMintConnector, AuthWallet};
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
    /// Mint server information (name, description, supported features, etc.)
    pub mint_info: MintInfo,

    /// All keysets indexed by their ID (includes both active and inactive)
    pub keysets: HashMap<Id, Arc<KeySetInfo>>,

    /// Cryptographic keys for each keyset, indexed by keyset ID
    pub keys: HashMap<Id, Arc<Keys>>,

    /// Subset of keysets that are currently active (cached for convenience)
    pub active_keysets: Vec<Arc<KeySetInfo>>,

    /// Freshness tracking for regular (non-auth) mint data
    status: FreshnessStatus,

    /// Freshness tracking for blind auth keysets (when `auth` feature enabled)
    #[cfg(feature = "auth")]
    auth_status: FreshnessStatus,
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

    /// Tracks which database instances have been synced to which cache version.
    /// Key: pointer identity of storage Arc, Value: last synced cache version
    db_sync_versions: Arc<RwLock<HashMap<usize, usize>>>,

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
    /// Sets the metadata cache TTL
    pub fn set_metadata_cache_ttl(&self, ttl: Option<Duration>) {
        let mut guarded_ttl = self.metadata_cache_ttl.write();
        *guarded_ttl = ttl;
    }

    /// Get information about metadata cache info
    pub fn get_metadata_cache_info(&self) -> FreshnessStatus {
        self.metadata_cache.metadata.load().status.clone()
    }
}

#[cfg(feature = "auth")]
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
    /// let cache = MintMetadataCache::new(mint_url, None);
    /// // No data loaded yet - call load() to fetch
    /// ```
    pub fn new(mint_url: MintUrl) -> Self {
        Self {
            mint_url,
            metadata: Arc::new(ArcSwap::default()),
            db_sync_versions: Arc::new(Default::default()),
            fetch_lock: Arc::new(Mutex::new(())),
        }
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
    #[inline(always)]
    pub async fn load_from_mint(
        &self,
        storage: &Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync>,
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

        // Load keys from database before fetching from HTTP
        // This prevents re-fetching keys we already have and avoids duplicate insertions
        if let Some(keysets) = storage.get_mint_keysets(self.mint_url.clone()).await? {
            let mut updated_metadata = (*self.metadata.load().clone()).clone();
            for keyset_info in keysets {
                if let Some(keys) = storage.get_keys(&keyset_info.id).await? {
                    tracing::trace!("Loaded keys for keyset {} from database", keyset_info.id);
                    updated_metadata.keys.insert(keyset_info.id, Arc::new(keys));
                }
            }
            // Update cache with database keys before HTTP fetch
            self.metadata.store(Arc::new(updated_metadata));
        }

        // Perform the fetch
        #[cfg(feature = "auth")]
        let metadata = self.fetch_from_http(Some(client), None).await?;

        #[cfg(not(feature = "auth"))]
        let metadata = self.fetch_from_http(Some(client)).await?;

        // Persist to database
        self.database_sync(storage.clone(), metadata.clone()).await;

        Ok(metadata)
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
    #[inline(always)]
    pub async fn load(
        &self,
        storage: &Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync>,
        client: &Arc<dyn MintConnector + Send + Sync>,
        ttl: Option<Duration>,
    ) -> Result<Arc<MintMetadata>, Error> {
        let cached_metadata = self.metadata.load().clone();
        let storage_id = Self::arc_pointer_id(storage);

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
            if db_synced_version != cached_metadata.status.version {
                // Database is stale - sync before returning
                self.database_sync(storage.clone(), cached_metadata.clone())
                    .await;
            }
            return Ok(cached_metadata);
        }

        // Cache not populated - fetch from mint
        self.load_from_mint(storage, client).await
    }

    /// Load auth keysets and keys (auth feature only)
    ///
    /// Fetches blind authentication keysets from the mint. Always performs
    /// an HTTP fetch to get current auth keysets.
    ///
    /// # Arguments
    ///
    /// * `storage` - Database to persist metadata to
    /// * `auth_client` - Auth-capable HTTP client for fetching blind auth keysets
    ///
    /// # Returns
    ///
    /// Metadata containing auth keysets and keys
    #[cfg(feature = "auth")]
    pub async fn load_auth(
        &self,
        storage: &Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync>,
        auth_client: &Arc<dyn AuthMintConnector + Send + Sync>,
    ) -> Result<Arc<MintMetadata>, Error> {
        let cached_metadata = self.metadata.load().clone();
        let storage_id = Self::arc_pointer_id(storage);

        let db_synced_version = self
            .db_sync_versions
            .read()
            .get(&storage_id)
            .cloned()
            .unwrap_or_default();

        // Check if auth data is populated in cache
        if cached_metadata.auth_status.is_populated
            && cached_metadata.auth_status.updated_at > Instant::now()
        {
            if db_synced_version != cached_metadata.status.version {
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
            && current_metadata.auth_status.updated_at > Instant::now()
        {
            tracing::debug!(
                "Auth cache was updated while waiting for fetch lock, returning cached data"
            );
            return Ok(current_metadata);
        }

        // Load keys from database before fetching from HTTP
        // This prevents re-fetching keys we already have and avoids duplicate insertions
        if let Some(keysets) = storage.get_mint_keysets(self.mint_url.clone()).await? {
            let mut updated_metadata = (*self.metadata.load().clone()).clone();
            for keyset_info in keysets {
                if let Some(keys) = storage.get_keys(&keyset_info.id).await? {
                    tracing::trace!(
                        "Loaded keys for keyset {} from database (auth)",
                        keyset_info.id
                    );
                    updated_metadata.keys.insert(keyset_info.id, Arc::new(keys));
                }
            }
            // Update cache with database keys before HTTP fetch
            self.metadata.store(Arc::new(updated_metadata));
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
        storage: Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync>,
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
        storage: Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync>,
        metadata: Arc<MintMetadata>,
        db_sync_versions: Arc<RwLock<HashMap<usize, usize>>>,
    ) {
        let storage_id = Self::arc_pointer_id(&storage);

        // Check if this write is still needed
        {
            let mut versions = db_sync_versions.write();

            let current_synced_version = versions.get(&storage_id).cloned().unwrap_or_default();

            if metadata.status.version <= current_synced_version {
                // A newer version has already been persisted - skip this write
                return;
            }

            // Mark this version as being synced
            versions.insert(storage_id, metadata.status.version);
        }

        // Save mint info
        storage
            .add_mint(mint_url.clone(), Some(metadata.mint_info.clone()))
            .await
            .inspect_err(|e| tracing::warn!("Failed to save mint info for {}: {}", mint_url, e))
            .ok();

        // Save all keysets
        let keysets: Vec<_> = metadata.keysets.values().map(|ks| (**ks).clone()).collect();

        if !keysets.is_empty() {
            storage
                .add_mint_keysets(mint_url.clone(), keysets)
                .await
                .inspect_err(|e| tracing::warn!("Failed to save keysets for {}: {}", mint_url, e))
                .ok();
        }

        // Save keys for each keyset
        for (keyset_id, keys) in &metadata.keys {
            if let Some(keyset_info) = metadata.keysets.get(keyset_id) {
                // Check if keys already exist in database to avoid duplicate insertion
                if storage.get_keys(keyset_id).await.ok().flatten().is_some() {
                    tracing::trace!(
                        "Keys for keyset {} already in database, skipping insert",
                        keyset_id
                    );
                    continue;
                }

                let keyset = KeySet {
                    id: *keyset_id,
                    unit: keyset_info.unit.clone(),
                    final_expiry: keyset_info.final_expiry,
                    keys: (**keys).clone(),
                };

                storage
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
                    .ok();
            }
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
    async fn fetch_from_http(
        &self,
        client: Option<&Arc<dyn MintConnector + Send + Sync>>,
        #[cfg(feature = "auth")] auth_client: Option<&Arc<dyn AuthMintConnector + Send + Sync>>,
    ) -> Result<Arc<MintMetadata>, Error> {
        tracing::debug!("Fetching mint metadata from HTTP for {}", self.mint_url);

        // Start with current cache to preserve data from other sources
        let mut new_metadata = (*self.metadata.load().clone()).clone();
        let mut keysets_to_fetch = Vec::new();

        // Fetch regular mint data
        if let Some(client) = client.as_ref() {
            // Get mint information
            new_metadata.mint_info = client.get_mint_info().await.inspect_err(|err| {
                tracing::error!("Failed to fetch mint info for {}: {}", self.mint_url, err);
            })?;

            // Get list of keysets
            keysets_to_fetch.extend(
                client
                    .get_mint_keysets()
                    .await
                    .inspect_err(|err| {
                        tracing::error!("Failed to fetch keysets for {}: {}", self.mint_url, err);
                    })?
                    .keysets,
            );
        }

        // Fetch auth keysets if auth client provided
        #[cfg(feature = "auth")]
        if let Some(auth_client) = auth_client.as_ref() {
            keysets_to_fetch.extend(auth_client.get_mint_blind_auth_keysets().await?.keysets);
        }

        tracing::debug!(
            "Fetched {} keysets for {}",
            keysets_to_fetch.len(),
            self.mint_url
        );

        // Fetch keys for each keyset
        for keyset_info in keysets_to_fetch {
            let keyset_arc = Arc::new(keyset_info.clone());
            new_metadata
                .keysets
                .insert(keyset_info.id, keyset_arc.clone());

            // Track active keysets separately for quick access
            if keyset_info.active {
                new_metadata.active_keysets.push(keyset_arc);
            }

            // Only fetch keys if we don't already have them cached
            if let std::collections::hash_map::Entry::Vacant(e) =
                new_metadata.keys.entry(keyset_info.id)
            {
                let keyset = if let Some(client) = client.as_ref() {
                    client.get_mint_keyset(keyset_info.id).await?
                } else {
                    #[cfg(feature = "auth")]
                    if let Some(auth_client) = auth_client.as_ref() {
                        auth_client
                            .get_mint_blind_auth_keyset(keyset_info.id)
                            .await?
                    } else {
                        return Err(Error::Internal);
                    }

                    #[cfg(not(feature = "auth"))]
                    return Err(Error::Internal);
                };

                // Verify the keyset ID matches the keys
                keyset.verify_id()?;

                e.insert(Arc::new(keyset.keys));
            }
        }

        // Update freshness status based on what was fetched
        if client.is_some() {
            new_metadata.status.is_populated = true;
            new_metadata.status.updated_at = Instant::now();
            new_metadata.status.version += 1;
        }

        #[cfg(feature = "auth")]
        if auth_client.is_some() {
            new_metadata.auth_status.is_populated = true;
            new_metadata.auth_status.updated_at = Instant::now();
            new_metadata.auth_status.version += 1;
        }

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

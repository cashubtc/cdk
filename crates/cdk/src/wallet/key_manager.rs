//! Centralized key management with in-memory caching
//!
//! Provides global key management for all wallets with automatic background refresh
//! and lock-free cache access. Keys are fetched from mint servers, cached in memory,
//! and periodically updated without blocking wallet operations.
//!
//! # Architecture
//!
//! - **Per-mint cache**: Stores keysets and keys indexed by ID with atomic updates
//! - **Background refresh**: Periodic 5-minute updates keep keys fresh
//! - **HTTP throttling**: Max 5 concurrent requests to prevent overwhelming servers
//! - **Database fallback**: Loads from storage when cache misses or HTTP fails
//!
//! # Usage
//!
//! ```ignore
//! let key_manager = KeyManager::new();
//! key_manager.register_mint(mint_url, unit, storage, client);
//! let keys = key_manager.get_keys(&mint_url, &keyset_id).await?;
//! ```

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use cdk_common::database::{self, WalletDatabase};
use cdk_common::mint_url::MintUrl;
use cdk_common::nuts::{KeySetInfo, Keys};
use cdk_common::parking_lot::{Mutex as ParkingLotMutex, RwLock as ParkingLotRwLock};
use cdk_common::task::spawn;
use cdk_common::util::unix_time;
use cdk_common::{KeySet, MintInfo};
use tokio::sync::{mpsc, Semaphore};
use tokio::task::JoinHandle;
use tracing::{debug, error, warn};

#[cfg(feature = "auth")]
use super::AuthMintConnector;
use crate::nuts::Id;
#[cfg(feature = "auth")]
use crate::wallet::AuthHttpClient;
use crate::wallet::MintConnector;
use crate::Error;

/// Refresh interval for background key refresh (5 minutes)
const DEFAULT_REFRESH_INTERVAL: Duration = Duration::from_secs(300);

/// Maximum concurrent HTTP requests to mint servers
const MAX_CONCURRENT_HTTP_REQUESTS: usize = 5;

const MAX_RETRY: usize = 50;
const RETRY_SLEEP: Duration = Duration::from_millis(100);

/// Manages refresh scheduling for mints
///
/// Tracks when each mint should be refreshed next. Uses BTreeMap for efficient
/// range queries to find all mints due for refresh. Counter ensures unique keys
/// when multiple mints are scheduled for the same instant.
#[derive(Clone)]
struct RefreshScheduler {
    /// Maps refresh time to mint URL
    schedule: Arc<ParkingLotMutex<BTreeMap<(Instant, usize), MintUrl>>>,

    /// Counter to ensure unique BTreeMap keys
    counter: Arc<AtomicUsize>,

    /// Interval between refreshes
    interval: Duration,
}

impl RefreshScheduler {
    /// Create a new refresh scheduler with the given interval
    fn new(interval: Duration) -> Self {
        Self {
            schedule: Arc::new(ParkingLotMutex::new(BTreeMap::new())),
            interval,
            counter: Arc::new(0.into()),
        }
    }

    /// Schedule a mint for refresh after the configured interval
    fn schedule_refresh(&self, mint_url: MintUrl) {
        let next_refresh = Instant::now() + self.interval;
        let mut schedule = self.schedule.lock();
        schedule.insert(
            (
                next_refresh,
                self.counter
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            ),
            mint_url,
        );
    }

    /// Get all mints that are due for refresh
    ///
    /// Returns and removes all mints whose scheduled refresh time has passed.
    fn get_due_refreshes(&self) -> Vec<MintUrl> {
        let mut schedule = self.schedule.lock();
        let now = Instant::now();

        let (keys, due_mints): (Vec<_>, Vec<_>) = schedule
            .range(..=(now, usize::MAX))
            .map(|(key, mint_url)| (*key, mint_url.clone()))
            .unzip();

        for key in keys {
            let _ = schedule.remove(&key);
        }

        due_mints
    }
}

/// Messages for the background refresh task
#[derive(Debug, Clone)]
pub enum RefreshMessage {
    /// Stop the refresh task
    Stop,

    /// Make sure the mint is loaded, from either any localstore or remotely.
    ///
    /// This function also make sure all stores have a copy of the mint info and keys
    SyncMint(MintUrl),

    /// Fetch keys for a specific mint immediately
    FetchMint(MintUrl),
}

/// Per-mint key cache
///
/// Stores all keyset and key data for a single mint. Updated atomically via ArcSwap.
/// The `refresh_version` increments on each update to detect when cache has changed.
#[derive(Clone, Debug)]
struct MintKeyCache {
    /// If the cache is ready
    is_ready: bool,

    /// Mint info from server
    mint_info: Option<MintInfo>,

    /// All keysets by ID
    keysets_by_id: HashMap<Id, Arc<KeySetInfo>>,

    /// Active keysets for quick access
    active_keysets: Vec<Arc<KeySetInfo>>,

    /// All keys by keyset ID
    keys_by_id: HashMap<Id, Arc<Keys>>,

    /// Last refresh timestamp
    last_refresh: Instant,

    /// Cache generation (increments on each refresh)
    refresh_version: u64,
}

impl MintKeyCache {
    fn empty() -> Self {
        Self {
            is_ready: false,
            mint_info: None,
            keysets_by_id: HashMap::new(),
            active_keysets: Vec::new(),
            keys_by_id: HashMap::new(),
            last_refresh: Instant::now(),
            refresh_version: 0,
        }
    }
}

/// External resources needed to manage keys for a mint
///
/// Combines storage and client into a single struct to keep them paired together.
#[derive(Clone)]
struct MintResources {
    /// Storage backend for persistence
    storage: Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync>,

    #[cfg(feature = "auth")]
    auth_client: Arc<dyn AuthMintConnector + Send + Sync>,

    /// Client for fetching keys from mint
    client: Arc<dyn MintConnector + Send + Sync>,
}

/// Per-mint registration info
///
/// Contains all resources and cached data for managing keys for a single mint.
#[derive(Clone)]
struct MintRegistration {
    /// Mint URL
    mint_url: MintUrl,

    /// External resources (storage + client)
    resources: Arc<ParkingLotRwLock<HashMap<usize, MintResources>>>,

    /// Cached data
    cache: Arc<ArcSwap<MintKeyCache>>,
}

impl std::fmt::Debug for MintRegistration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MintRegistration")
            .field("mint_url", &self.mint_url)
            .field("resources", &"<MintResources>")
            .field("cache", &self.cache)
            .finish()
    }
}

type Mints = Arc<ParkingLotRwLock<HashMap<MintUrl, MintRegistration>>>;

/// Global key manager shared across all wallets
///
/// Centralizes key management with in-memory caching and background refresh.
/// All wallets share the same cache for each mint, avoiding duplicate fetches.
///
/// Spawns a background task on creation that refreshes keys every 5 minutes.
/// Dropping the KeyManager stops the background task.
pub struct KeyManager {
    /// Registered mints by URL (using parking_lot for sync access)
    mints: Mints,

    /// Message sender to refresh task
    tx: mpsc::UnboundedSender<RefreshMessage>,

    /// Background refresh task handle
    refresh_task: Arc<ParkingLotMutex<Option<JoinHandle<()>>>>,

    /// Refresh interval
    refresh_interval: Duration,

    /// Internal counter for each registered/mint wallet
    counter: AtomicUsize,

    /// Semaphore to limit concurrent HTTP requests to mint servers
    /// This is stored to keep it alive and is passed to the refresh loop
    #[allow(dead_code)]
    refresh_semaphore: Arc<Semaphore>,
}

impl std::fmt::Debug for KeyManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyManager")
            .field("mints", &self.mints)
            .field("refresh_interval", &self.refresh_interval)
            .finish()
    }
}

/// KeySubscription
pub struct KeySubscription {
    /// Registered mints by URL (using parking_lot for sync access)
    mints: Mints,
    mint_url: MintUrl,
    id: usize,
}

impl std::fmt::Debug for KeySubscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyManager")
            .field("mint_url", &self.mint_url)
            .finish()
    }
}

impl Drop for KeySubscription {
    fn drop(&mut self) {
        KeyManager::deregister_mint(self.mints.clone(), self.mint_url.clone(), self.id);
    }
}

impl KeyManager {
    /// Create a new KeyManager with default 5-minute refresh interval
    ///
    /// Spawns a background task that refreshes all registered mints periodically.
    pub fn new() -> Arc<Self> {
        Self::with_refresh_interval(DEFAULT_REFRESH_INTERVAL)
    }

    /// Create a new KeyManager with custom refresh interval
    pub fn with_refresh_interval(refresh_interval: Duration) -> Arc<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        let refresh_semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_HTTP_REQUESTS));

        let manager = Self {
            mints: Arc::new(ParkingLotRwLock::new(HashMap::new())),
            tx,
            refresh_task: Arc::new(ParkingLotMutex::new(None)),
            refresh_interval,
            refresh_semaphore: refresh_semaphore.clone(),
            counter: 0.into(),
        };

        let mints = manager.mints.clone();
        let refresh_interval = manager.refresh_interval;
        let task = spawn(async move {
            Self::refresh_loop(rx, mints, refresh_interval, refresh_semaphore).await;
        });

        {
            let mut refresh_task = manager.refresh_task.lock();
            *refresh_task = Some(task);
        }

        Arc::new(manager)
    }

    /// Send a message to the background refresh task
    fn send_message(&self, msg: RefreshMessage) {
        let _ = self
            .tx
            .send(msg)
            .inspect_err(|e| error!("Failed to send message to refresh task: {}", e));
    }

    /// Register a mint for key management
    ///
    /// Registers the mint immediately and triggers an initial key fetch in the background.
    /// The mint will be automatically refreshed every `refresh_interval`.
    pub fn register_mint(
        &self,
        mint_url: MintUrl,
        storage: Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync>,
        client: Arc<dyn MintConnector + Send + Sync>,
    ) -> Arc<KeySubscription> {
        debug!("Registering mint: {}", mint_url);
        let mut mints = self.mints.write();

        let mint = mints.entry(mint_url.clone()).or_insert_with(|| {
            let cache = Arc::new(ArcSwap::from_pointee(MintKeyCache::empty()));

            MintRegistration {
                mint_url: mint_url.clone(),
                resources: Arc::new(ParkingLotRwLock::new(HashMap::new())),
                cache,
            }
        });

        let id = self
            .counter
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);

        #[cfg(feature = "auth")]
        let mint_resource = MintResources {
            storage,
            client,
            auth_client: Arc::new(AuthHttpClient::new(mint_url.clone(), None)),
        };

        #[cfg(not(feature = "auth"))]
        let mint_resource = MintResources { storage, client };

        mint.resources.write().insert(id, mint_resource);

        drop(mints);

        debug!("Mint registered: {}", mint_url);

        self.send_message(RefreshMessage::SyncMint(mint_url.clone()));

        Arc::new(KeySubscription {
            mints: self.mints.clone(),
            mint_url,
            id,
        })
    }

    fn deregister_mint(locked_mints: Mints, mint_url: MintUrl, internal_id: usize) {
        let mut mints = locked_mints.write();
        let mint = if let Some(r) = mints.remove(&mint_url) {
            r
        } else {
            return;
        };
        let mut r = mint.resources.write();
        r.remove(&internal_id);

        if !r.is_empty() {
            // add mint back, as there are other wallets to the same mint_url active
            drop(r);
            mints.insert(mint_url, mint);
        }
    }

    /// Get keys for a keyset (cache-first with automatic refresh)
    ///
    /// Returns keys from cache if available. If not cached, triggers a refresh
    /// and waits up to 2 seconds for the keys to arrive.
    pub async fn get_keys(&self, mint_url: &MintUrl, keyset_id: &Id) -> Result<Arc<Keys>, Error> {
        let shared_cache = {
            let mints = self.mints.read();
            let registration = mints.get(mint_url).ok_or(Error::IncorrectMint)?;
            registration.cache.clone()
        };

        for _ in 0..MAX_RETRY {
            let cache = shared_cache.load();
            if cache.is_ready {
                return cache
                    .keys_by_id
                    .get(keyset_id)
                    .cloned()
                    .ok_or(Error::UnknownKeySet);
            }
            self.send_message(RefreshMessage::SyncMint(mint_url.to_owned()));
            tokio::time::sleep(RETRY_SLEEP).await;
        }

        Err(Error::UnknownKeySet)
    }

    /// Get keyset info by ID (cache-first with automatic refresh)
    pub async fn get_keyset_by_id(
        &self,
        mint_url: &MintUrl,
        keyset_id: &Id,
    ) -> Result<Arc<KeySetInfo>, Error> {
        let shared_cache = {
            let mints = self.mints.read();
            let registration = mints.get(mint_url).ok_or(Error::IncorrectMint)?;
            registration.cache.clone()
        };

        for _ in 0..MAX_RETRY {
            let cache = shared_cache.load();
            if cache.is_ready {
                return cache
                    .keysets_by_id
                    .get(keyset_id)
                    .cloned()
                    .ok_or(Error::UnknownKeySet);
            }
            self.send_message(RefreshMessage::SyncMint(mint_url.to_owned()));
            tokio::time::sleep(RETRY_SLEEP).await;
        }

        Err(Error::UnknownKeySet)
    }

    /// Get all keysets for a mint (cache-first with automatic refresh)
    pub async fn get_keysets(&self, mint_url: &MintUrl) -> Result<Vec<KeySetInfo>, Error> {
        let shared_cache = {
            let mints = self.mints.read();
            let registration = mints.get(mint_url).ok_or(Error::IncorrectMint)?;
            registration.cache.clone()
        };

        for _ in 0..MAX_RETRY {
            let cache = shared_cache.load();
            if cache.is_ready {
                let keysets: Vec<KeySetInfo> = cache
                    .keysets_by_id
                    .values()
                    .map(|ks| (**ks).clone())
                    .collect();
                return if keysets.is_empty() {
                    Err(Error::UnknownKeySet)
                } else {
                    Ok(keysets)
                };
            }

            self.send_message(RefreshMessage::SyncMint(mint_url.to_owned()));
            tokio::time::sleep(RETRY_SLEEP).await;
        }

        Err(Error::UnknownKeySet)
    }

    /// Get all active keysets for a mint (cache-only, no refresh)
    pub async fn get_active_keysets(
        &self,
        mint_url: &MintUrl,
    ) -> Result<Vec<Arc<KeySetInfo>>, Error> {
        let shared_cache = {
            let mints = self.mints.read();
            let registration = mints.get(mint_url).ok_or(Error::IncorrectMint)?;
            registration.cache.clone()
        };

        for _ in 0..MAX_RETRY {
            let cache = shared_cache.load();
            if cache.is_ready {
                return Ok(cache.active_keysets.clone());
            }
            self.send_message(RefreshMessage::SyncMint(mint_url.to_owned()));
            tokio::time::sleep(RETRY_SLEEP).await;
        }

        Err(Error::UnknownKeySet)
    }

    /// Load a specific keyset from database or HTTP
    ///
    /// First checks all registered databases for the keyset. If not found,
    /// fetches from the mint server via HTTP and persists to all databases.
    async fn load_keyset_from_db_or_http(
        registration: &MintRegistration,
        keyset_id: &Id,
    ) -> Result<KeySet, Error> {
        let storages = registration
            .resources
            .read()
            .values()
            .map(|resource| resource.storage.clone())
            .collect::<Vec<_>>();

        // Try database first
        for storage in &storages {
            if let Some(keys) = storage.get_keys(keyset_id).await? {
                debug!(
                    "Loaded keyset {} from database for {}",
                    keyset_id, registration.mint_url
                );

                // Get keyset info to construct KeySet
                if let Some(keyset_info) = storage.get_keyset_by_id(keyset_id).await? {
                    return Ok(KeySet {
                        id: *keyset_id,
                        unit: keyset_info.unit,
                        final_expiry: keyset_info.final_expiry,
                        keys,
                    });
                }
            }
        }

        // Not in database, fetch from HTTP
        debug!(
            "Keyset {} not in database, fetching from mint server for {}",
            keyset_id, registration.mint_url
        );

        let http_client = registration
            .resources
            .read()
            .values()
            .next()
            .ok_or(Error::IncorrectMint)?
            .client
            .clone();

        let keyset = http_client.get_mint_keyset(*keyset_id).await?;

        // Persist to all databases
        for storage in &storages {
            let _ = storage.add_keys(keyset.clone()).await.inspect_err(|e| {
                warn!(
                    "Failed to persist keyset {} for {}: {}",
                    keyset_id, registration.mint_url, e
                )
            });
        }

        debug!(
            "Loaded keyset {} from HTTP for {}",
            keyset_id, registration.mint_url
        );

        Ok(keyset)
    }

    /// Load mint info and keys from all registered databases
    ///
    /// Iterates through all storage backends and loads keysets and keys into cache.
    /// This is called on first access when cache is empty.
    async fn fetch_mint_info_and_keys_from_db(
        registration: &MintRegistration,
    ) -> Result<(), Error> {
        debug!(
            "Cache empty, loading from storage first for {}",
            registration.mint_url
        );

        let mut storage_cache = MintKeyCache::empty();
        let storages = registration
            .resources
            .read()
            .values()
            .map(|resource| resource.storage.clone())
            .collect::<Vec<_>>();

        for storage in storages {
            if storage_cache.mint_info.is_none() {
                storage_cache.mint_info = storage.get_mint(registration.mint_url.clone()).await?;
            }

            for keyset in storage
                .get_mint_keysets(registration.mint_url.clone())
                .await?
                .ok_or(Error::UnknownKeySet)?
            {
                if storage_cache.keysets_by_id.contains_key(&keyset.id) {
                    continue;
                }

                let arc_keyset = Arc::new(keyset.clone());
                storage_cache
                    .keysets_by_id
                    .insert(keyset.id, arc_keyset.clone());

                if keyset.active {
                    storage_cache.active_keysets.push(arc_keyset);
                }
            }

            for keyset_id in storage_cache.keysets_by_id.keys() {
                if storage_cache.keys_by_id.contains_key(keyset_id) {
                    continue;
                }

                if let Some(keys) = storage.get_keys(keyset_id).await? {
                    storage_cache.keys_by_id.insert(*keyset_id, Arc::new(keys));
                }
            }
        }

        let keys_count = storage_cache.keys_by_id.len();
        storage_cache.refresh_version += 1;
        storage_cache.is_ready = true;

        let storage_cache = Arc::new(storage_cache);
        registration.cache.store(storage_cache.clone());

        Self::persist_cache(registration, storage_cache).await;

        debug!(
            "Loaded {} keys from storage for {}",
            keys_count, registration.mint_url
        );

        Ok(())
    }

    /// Persist cache to a single database
    ///
    /// Writes mint info, keysets, and keys to the given storage backend.
    /// Errors are logged but don't fail the operation.
    async fn persist_cache_db(
        storage: Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync>,
        mint_url: MintUrl,
        new_cache: Arc<MintKeyCache>,
    ) {
        if !new_cache.is_ready {
            return;
        }

        if new_cache.mint_info.is_some() {
            let _ = storage
                .add_mint(mint_url.clone(), new_cache.mint_info.clone())
                .await
                .inspect_err(|e| {
                    warn!("Failed to persist mint_info for {}: {}", mint_url, e);
                });
        }
        let _ = storage
            .add_mint_keysets(
                mint_url.clone(),
                new_cache
                    .keysets_by_id
                    .values()
                    .map(|ks| (**ks).clone())
                    .collect(),
            )
            .await
            .inspect_err(|e| warn!("Failed to persist keysets for {}: {}", mint_url, e));

        for (keyset_id, keys) in new_cache.keys_by_id.iter() {
            if storage
                .get_keys(keyset_id)
                .await
                .inspect_err(|e| warn!("Failed to get_keys {e}"))
                .unwrap_or_default()
                .is_none()
            {
                let keyset = if let Some(v) = new_cache.keysets_by_id.get(keyset_id) {
                    v
                } else {
                    warn!("Malformed keysets, cannot find {}", keyset_id);
                    continue;
                };
                let _ = storage
                    .add_keys(KeySet {
                        id: *keyset_id,
                        unit: keyset.unit.clone(),
                        final_expiry: keyset.final_expiry,
                        keys: (**keys).clone(),
                    })
                    .await
                    .inspect_err(|e| {
                        warn!("Failed to persist keys for keyset {}: {}", keyset_id, e)
                    });
            }
        }
    }

    /// Persist cache to all registered databases
    ///
    /// Spawns a task for each storage backend to write cache asynchronously.
    async fn persist_cache(registration: &MintRegistration, new_cache: Arc<MintKeyCache>) {
        let storages = registration
            .resources
            .read()
            .values()
            .map(|x| x.storage.clone())
            .collect::<Vec<_>>();

        for storage in storages {
            spawn(Self::persist_cache_db(
                storage,
                registration.mint_url.clone(),
                new_cache.clone(),
            ));
        }
    }

    /// Fetch keys from mint server via HTTP
    ///
    /// Fetches mint info, keysets, and keys from the mint server. Updates cache
    /// and schedules next refresh. Persists new data to all databases.
    async fn fetch_from_http(
        registration: MintRegistration,
        refresh_scheduler: RefreshScheduler,
    ) -> Result<(), Error> {
        debug!(
            "Fetching keys from mint server for {}",
            registration.mint_url
        );

        let http_client = registration
            .resources
            .read()
            .values()
            .next()
            .ok_or(Error::IncorrectMint)?
            .client
            .clone();

        #[cfg(feature = "auth")]
        let http_auth_client = registration
            .resources
            .read()
            .values()
            .next()
            .ok_or(Error::IncorrectMint)?
            .auth_client
            .clone();

        let mint_info = http_client.get_mint_info().await?;

        if let Some(mint_unix_time) = mint_info.time {
            let current_unix_time = unix_time();
            if current_unix_time.abs_diff(mint_unix_time) > 30 {
                tracing::warn!(
                    "Mint time does match wallet time. Mint: {}, Wallet: {}",
                    mint_unix_time,
                    current_unix_time
                );
                return Err(Error::MintTimeExceedsTolerance);
            }
        }

        let keysets_response = http_client.get_mint_keysets().await?;

        let keysets = keysets_response.keysets;

        #[cfg(feature = "auth")]
        let keysets = if let Ok(auth_keysets_response) =
            http_auth_client.get_mint_blind_auth_keysets().await
        {
            let mut keysets = keysets;
            keysets.extend_from_slice(&auth_keysets_response.keysets);
            keysets
        } else {
            keysets
        };

        let mut new_cache = MintKeyCache::empty();

        for keyset_info in keysets {
            let arc_keyset = Arc::new(keyset_info.clone());
            new_cache
                .keysets_by_id
                .insert(keyset_info.id, arc_keyset.clone());

            if keyset_info.active {
                new_cache.active_keysets.push(arc_keyset);
            }

            // Try to load keyset from database first, then HTTP
            if let Ok(keyset) = Self::load_keyset_from_db_or_http(&registration, &keyset_info.id)
                .await
                .inspect_err(|e| {
                    warn!(
                        "Failed to load keyset {} for {}: {}",
                        keyset_info.id, registration.mint_url, e
                    )
                })
            {
                let keys = Arc::new(keyset.keys.clone());
                new_cache.keys_by_id.insert(keyset_info.id, keys);
            }
        }

        refresh_scheduler.schedule_refresh(registration.mint_url.clone());

        let old_generation = registration.cache.load().refresh_version;
        new_cache.mint_info = Some(mint_info);
        new_cache.refresh_version = old_generation + 1;
        new_cache.is_ready = true;
        new_cache.last_refresh = Instant::now();

        debug!(
            "Refreshed {} keysets and {} keys for {} (generation {})",
            new_cache.keysets_by_id.len(),
            new_cache.keys_by_id.len(),
            registration.mint_url,
            new_cache.refresh_version
        );

        let new_cache = Arc::new(new_cache);
        Self::persist_cache(&registration, new_cache.clone()).await;
        registration.cache.store(new_cache);

        Ok::<(), Error>(())
    }

    fn sync_mint_task(registration: MintRegistration, refresh_scheduler: RefreshScheduler) {
        spawn(async move {
            if !registration.cache.load().is_ready {
                let _ = Self::fetch_mint_info_and_keys_from_db(&registration)
                    .await
                    .inspect_err(|e| {
                        warn!(
                            "Failed to load keys from storage for {}: {}",
                            registration.mint_url, e
                        )
                    });
            }

            if !registration.cache.load().is_ready {
                let mint_url = registration.mint_url.clone();
                let _ = tokio::time::timeout(
                    Duration::from_secs(60),
                    Self::fetch_from_http(registration, refresh_scheduler),
                )
                .await
                .inspect_err(|e| warn!("Failed to fetch keys for {} with error {}", mint_url, e));
            }
        });
    }

    /// Refresh keys from mint server
    ///
    /// Spawns an async task with 60s timeout. HTTP requests are limited to
    /// MAX_CONCURRENT_HTTP_REQUESTS concurrent requests via semaphore.
    fn fetch_and_sync_mint_task(
        registration: MintRegistration,
        semaphore: Arc<Semaphore>,
        refresh_scheduler: RefreshScheduler,
    ) {
        spawn(async move {
            if !registration.cache.load().is_ready {
                let _ = Self::fetch_mint_info_and_keys_from_db(&registration)
                    .await
                    .inspect_err(|e| {
                        warn!(
                            "Failed to load keys from storage for {}: {}",
                            registration.mint_url, e
                        )
                    });
            }

            let Ok(http_permit) = semaphore.acquire().await.inspect_err(|e| {
                error!(
                    "Failed to acquire HTTP permit for {}: {}",
                    registration.mint_url, e
                );
            }) else {
                return;
            };

            debug!(
                "Acquired HTTP permit for {} ({} available)",
                registration.mint_url,
                semaphore.available_permits()
            );

            let mint_url = registration.mint_url.clone();

            let result = tokio::time::timeout(
                Duration::from_secs(60),
                Self::fetch_from_http(registration, refresh_scheduler),
            )
            .await;

            drop(http_permit);

            let _ = result
                .map_err(|_| Error::Timeout)
                .and_then(|r| r)
                .inspect(|_| {
                    debug!(
                        "Successfully fetched keys from mint server for {}",
                        mint_url
                    );
                })
                .inspect_err(|e| match e {
                    Error::Timeout => {
                        error!("Timeout fetching keys from mint server for {}", mint_url)
                    }
                    _ => {
                        error!(
                            "Failed to fetch keys from mint server for {}: {}",
                            mint_url, e
                        )
                    }
                });

            debug!(
                "Released HTTP permit for {} ({} available)",
                mint_url,
                semaphore.available_permits()
            );
        });
    }

    /// Background refresh loop with message handling
    ///
    /// Runs independently and handles refresh messages and periodic updates.
    /// All refresh operations are spawned as separate tasks with timeouts.
    async fn refresh_loop(
        mut rx: mpsc::UnboundedReceiver<RefreshMessage>,
        mints: Arc<ParkingLotRwLock<HashMap<MintUrl, MintRegistration>>>,
        refresh_interval: Duration,
        semaphore: Arc<Semaphore>,
    ) {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        let refresh_scheduler = RefreshScheduler::new(refresh_interval);

        loop {
            tokio::select! {
                Some(msg) = rx.recv() => {
                    match msg {
                        RefreshMessage::Stop => {
                            debug!("Stopping refresh task");
                            break;
                        }
                        RefreshMessage::SyncMint(mint_url) => {
                            debug!("Sync all instances of {}", mint_url);
                            let registration = {
                                let mints_lock = mints.read();
                                mints_lock.get(&mint_url).cloned()
                            };

                            if let Some(reg) = registration {
                                Self::sync_mint_task(reg, refresh_scheduler.clone());
                            } else {
                                warn!("FetchMint: Mint not registered: {}", mint_url);
                            }
                        }
                        RefreshMessage::FetchMint(mint_url) => {
                            debug!("FetchMint message received for {}", mint_url);
                            let registration = {
                                let mints_lock = mints.read();
                                mints_lock.get(&mint_url).cloned()
                            };

                            if let Some(reg) = registration {
                                Self::fetch_and_sync_mint_task(reg, semaphore.clone(), refresh_scheduler.clone());
                            } else {
                                warn!("FetchMint: Mint not registered: {}", mint_url);
                            }
                        }
                    }
                }

                _ = interval.tick() => {
                    debug!("Checking for mints due for refresh");

                    let due_mints = refresh_scheduler.get_due_refreshes();

                    if !due_mints.is_empty() {
                        debug!("Found {} mints due for refresh", due_mints.len());
                    }

                    for mint_url in due_mints {
                        let registration = {
                            let mints_lock = mints.read();
                            mints_lock.get(&mint_url).cloned()
                        };

                        if let Some(reg) = registration {
                            Self::fetch_and_sync_mint_task(reg, semaphore.clone(), refresh_scheduler.clone());
                        } else {
                            warn!("Mint no longer registered: {}", mint_url);
                        }
                    }
                }
            }
        }

        debug!("Refresh loop stopped");
    }

    /// Trigger a refresh and wait for it to complete
    ///
    /// Sends a refresh message to the background task and waits up to 2 seconds
    /// for the cache to be updated with a newer version.
    pub async fn refresh(&self, mint_url: &MintUrl) -> Result<Vec<KeySetInfo>, Error> {
        let shared_cache = {
            let mints = self.mints.read();
            let registration = mints.get(mint_url).ok_or(Error::IncorrectMint)?;
            registration.cache.clone()
        };

        let last_version = shared_cache.load().refresh_version;

        self.send_message(RefreshMessage::FetchMint(mint_url.clone()));

        for _ in 0..MAX_RETRY {
            if let Some(keysets) = {
                let cache = shared_cache.load();
                if last_version > 0 || cache.refresh_version > 0 {
                    Some(
                        cache
                            .keysets_by_id
                            .values()
                            .map(|ks| (**ks).clone())
                            .collect::<Vec<_>>(),
                    )
                } else {
                    None
                }
            } {
                return Ok(keysets);
            }

            tokio::time::sleep(RETRY_SLEEP).await;
        }

        Err(Error::UnknownKeySet)
    }

    /// Trigger a refresh for a specific mint (non-blocking)
    pub fn refresh_now(&self, mint_url: &MintUrl) {
        self.send_message(RefreshMessage::FetchMint(mint_url.clone()));
    }
}

impl Drop for KeyManager {
    fn drop(&mut self) {
        self.send_message(RefreshMessage::Stop);
        if let Some(mut task) = self.refresh_task.try_lock() {
            if let Some(handle) = task.take() {
                handle.abort();
            }
        }
    }
}

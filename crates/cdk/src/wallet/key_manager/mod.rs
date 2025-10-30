//! Per-mint key management with in-memory caching
//!
//! Provides key management for individual mints with automatic background refresh
//! and lock-free cache access. Keys are fetched from mint servers, cached in memory,
//! and periodically updated without blocking wallet operations.
//!
//! # Architecture
//!
//! - **Per-mint cache**: Stores keysets and keys indexed by ID with atomic updates
//! - **Background refresh**: Periodic 5-minute updates keep keys fresh
//! - **Database fallback**: Loads from storage when cache misses or HTTP fails
//!
//! # Usage
//!
//! ```ignore
//! let key_manager = Arc::new(KeyManager::new(mint_url, storage, client));
//! let keys = key_manager.get_keys(&keyset_id).await?;
//! ```

use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use cdk_common::database::{self, WalletDatabase};
use cdk_common::mint_url::MintUrl;
use cdk_common::nuts::{KeySetInfo, Keys};
use cdk_common::task::spawn;
use cdk_common::MintInfo;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use worker::MessageToWorker;

mod worker;

use crate::nuts::Id;
#[cfg(feature = "auth")]
use crate::wallet::AuthHttpClient;
use crate::wallet::MintConnector;
use crate::Error;

/// Refresh interval for background key refresh (5 minutes)
const DEFAULT_REFRESH_INTERVAL: Duration = Duration::from_secs(300);

const MAX_RETRY: usize = 50;
const RETRY_SLEEP: Duration = Duration::from_millis(100);

/// Per-mint key cache
///
/// Stores all keyset and key data for a single mint. Updated atomically via ArcSwap.
/// The `refresh_version` increments on each update to detect when cache has changed.
#[derive(Clone, Debug)]
pub(super) struct MintKeyCache {
    /// If the cache is ready
    pub is_ready: bool,

    /// Mint info from server
    pub mint_info: Option<MintInfo>,

    /// All keysets by ID
    pub keysets_by_id: HashMap<Id, Arc<KeySetInfo>>,

    /// Active keysets for quick access
    pub active_keysets: Vec<Arc<KeySetInfo>>,

    /// All keys by keyset ID
    pub keys_by_id: HashMap<Id, Arc<Keys>>,

    /// Last refresh timestamp
    pub last_refresh: Instant,

    /// Cache generation (increments on each refresh)
    pub refresh_version: u64,
}

impl MintKeyCache {
    pub(super) fn empty() -> Self {
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

/// Key manager for a single mint
///
/// Manages keys for a specific mint with in-memory caching and background refresh.
/// Each KeyManager owns its background worker task.
pub struct KeyManager {
    /// Mint URL
    mint_url: MintUrl,

    /// Storage backend
    #[allow(dead_code)]
    storage: Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync>,

    /// Shared cache (atomic updates)
    cache: Arc<ArcSwap<MintKeyCache>>,

    /// Message sender to background worker
    tx: mpsc::Sender<MessageToWorker>,

    /// Background worker task handle
    task: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for KeyManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyManager")
            .field("mint_url", &self.mint_url)
            .field("cache_ready", &self.cache.load().is_ready)
            .finish()
    }
}

impl Drop for KeyManager {
    fn drop(&mut self) {
        tracing::debug!("Dropping KeyManager for {}", self.mint_url);
        self.tx
            .try_send(MessageToWorker::Stop)
            .inspect_err(|e| {
                tracing::error!("Failed to send Stop message for {}: {}", self.mint_url, e)
            })
            .ok();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl KeyManager {
    /// Create a new KeyManager with default 5-minute refresh interval
    pub fn new(
        mint_url: MintUrl,
        storage: Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync>,
        client: Arc<dyn MintConnector + Send + Sync>,
    ) -> Self {
        Self::with_refresh_interval(mint_url, storage, client, DEFAULT_REFRESH_INTERVAL)
    }

    /// Create a new KeyManager with custom refresh interval
    pub fn with_refresh_interval(
        mint_url: MintUrl,
        storage: Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync>,
        client: Arc<dyn MintConnector + Send + Sync>,
        refresh_interval: Duration,
    ) -> Self {
        tracing::debug!("Creating KeyManager for mint: {}", mint_url);

        let (tx, rx) = mpsc::channel(1_000);
        let cache = Arc::new(ArcSwap::from_pointee(MintKeyCache::empty()));

        #[cfg(feature = "auth")]
        let auth_client = Arc::new(AuthHttpClient::new(mint_url.clone(), None));

        let task = {
            let mint_url_clone = mint_url.clone();
            let client_clone = client.clone();
            #[cfg(feature = "auth")]
            let auth_client_clone = auth_client.clone();
            let storage_clone = storage.clone();
            let cache_clone = cache.clone();

            spawn(worker::refresh_loop(
                mint_url_clone,
                client_clone,
                #[cfg(feature = "auth")]
                auth_client_clone,
                storage_clone,
                cache_clone,
                rx,
                refresh_interval,
            ))
        };

        // Trigger initial sync (best effort - log if it fails)
        if let Err(e) = tx.try_send(MessageToWorker::FetchMint) {
            tracing::error!(
                "Failed to send initial FetchMint message for {}: {}",
                mint_url,
                e
            );
        }

        Self {
            mint_url,
            storage,
            cache,
            tx,
            task: Some(task),
        }
    }

    /// Get the mint URL for this KeyManager
    pub fn mint_url(&self) -> &MintUrl {
        &self.mint_url
    }

    /// Send a message to the background refresh task (best effort)
    fn send_message(&self, msg: MessageToWorker) {
        if let Err(e) = self.tx.try_send(msg) {
            tracing::error!(
                "Failed to send message to refresh task for {} (closed: {}): {}",
                self.mint_url,
                self.tx.is_closed(),
                e
            );
        }
    }

    /// Get keys for a keyset (cache-first with automatic refresh)
    ///
    /// Returns keys from cache if available. If not cached, triggers a refresh
    /// and waits up to 5 seconds for the keys to arrive.
    pub async fn get_keys(&self, keyset_id: &Id) -> Result<Arc<Keys>, Error> {
        for _ in 0..MAX_RETRY {
            let cache = self.cache.load();
            if cache.is_ready {
                return cache
                    .keys_by_id
                    .get(keyset_id)
                    .cloned()
                    .ok_or(Error::UnknownKeySet);
            }
            tokio::time::sleep(RETRY_SLEEP).await;
        }

        Err(Error::UnknownKeySet)
    }

    /// Get keyset info by ID (cache-first with automatic refresh)
    pub async fn get_keyset_by_id(&self, keyset_id: &Id) -> Result<Arc<KeySetInfo>, Error> {
        for _ in 0..MAX_RETRY {
            let cache = self.cache.load();
            if cache.is_ready {
                return cache
                    .keysets_by_id
                    .get(keyset_id)
                    .cloned()
                    .ok_or(Error::UnknownKeySet);
            }
            tokio::time::sleep(RETRY_SLEEP).await;
        }

        Err(Error::UnknownKeySet)
    }

    /// Get all keysets for the mint (cache-first with automatic refresh)
    pub async fn get_keysets(&self) -> Result<Vec<KeySetInfo>, Error> {
        for _ in 0..MAX_RETRY {
            let cache = self.cache.load();
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

            tokio::time::sleep(RETRY_SLEEP).await;
        }

        Err(Error::UnknownKeySet)
    }

    /// Get all active keysets for the mint (cache-first with automatic refresh)
    pub async fn get_active_keysets(&self) -> Result<Vec<Arc<KeySetInfo>>, Error> {
        for _ in 0..MAX_RETRY {
            let cache = self.cache.load();
            if cache.is_ready {
                return Ok(cache.active_keysets.clone());
            }
            tokio::time::sleep(RETRY_SLEEP).await;
        }

        Err(Error::UnknownKeySet)
    }

    /// Trigger a refresh and wait for it to complete
    ///
    /// Sends a refresh message to the background task and waits up to 5 seconds
    /// for the cache to be updated with a newer version.
    pub async fn refresh(&self) -> Result<Vec<KeySetInfo>, Error> {
        let last_version = self.cache.load().refresh_version;

        self.send_message(MessageToWorker::FetchMint);

        for _ in 0..MAX_RETRY {
            if let Some(keysets) = {
                let cache = self.cache.load();
                if last_version < cache.refresh_version {
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

    /// Trigger a refresh (non-blocking)
    pub fn refresh_now(&self) {
        self.send_message(MessageToWorker::FetchMint);
    }
}

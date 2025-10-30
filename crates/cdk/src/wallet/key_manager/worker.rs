//! Background worker for fetching and refreshing mint keys

use std::fmt::Debug;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use cdk_common::database::{self, WalletDatabase};
use cdk_common::mint_url::MintUrl;
use cdk_common::KeySet;
use tokio::time::sleep;

use super::MintKeyCache;
use crate::nuts::Id;
#[cfg(feature = "auth")]
use crate::wallet::AuthMintConnector;
use crate::wallet::MintConnector;
use crate::Error;

/// Messages for the background refresh task
#[derive(Debug)]
pub(super) enum MessageToWorker {
    /// Stop the refresh task
    Stop,

    /// Fetch keys from the mint immediately
    FetchMint,
}

/// Load a specific keyset from database or HTTP
///
/// First checks the database for the keyset. If not found,
/// fetches from the mint server via HTTP and persists to database.
async fn load_keyset_from_db_or_http(
    mint_url: &MintUrl,
    client: &Arc<dyn MintConnector + Send + Sync>,
    storage: &Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync>,
    keyset_id: &Id,
) -> Result<KeySet, Error> {
    // Try database first
    if let Some(keys) = storage.get_keys(keyset_id).await? {
        tracing::debug!("Loaded keyset {} from database for {}", keyset_id, mint_url);

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

    // Not in database, fetch from HTTP
    tracing::debug!(
        "Keyset {} not in database, fetching from mint server for {}",
        keyset_id,
        mint_url
    );

    let keyset = client.get_mint_keyset(*keyset_id).await?;
    keyset.verify_id()?;

    // Persist to database
    storage.add_keys(keyset.clone()).await.inspect_err(|e| {
        tracing::warn!(
            "Failed to persist keyset {} for {}: {}",
            keyset_id,
            mint_url,
            e
        )
    })?;

    tracing::debug!("Loaded keyset {} from HTTP for {}", keyset_id, mint_url);

    Ok(keyset)
}

/// Load cached mint data from storage backend
///
/// Loads keysets and keys from storage.
/// Marks cache as ready only if mint_info was found.
///
/// Returns a MintKeyCache that may or may not be ready depending on what was found.
async fn load_cache_from_storage(
    mint_url: &MintUrl,
    storage: &Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync>,
) -> Result<MintKeyCache, Error> {
    tracing::debug!("Loading cache from storage for {}", mint_url);

    let mut cache = MintKeyCache::empty();

    // Load mint info
    match storage.get_mint(mint_url.clone()).await {
        Ok(Some(mint_info)) => {
            tracing::debug!("Found mint info in storage for {}", mint_url);
            cache.mint_info = Some(mint_info);
        }
        Ok(None) => {
            tracing::debug!("No mint info in storage for {}", mint_url);
        }
        Err(e) => {
            tracing::warn!(
                "Error loading mint info from storage for {}: {}",
                mint_url,
                e
            );
        }
    }

    // Load keysets
    match storage.get_mint_keysets(mint_url.clone()).await {
        Ok(Some(keysets)) if !keysets.is_empty() => {
            tracing::debug!(
                "Loaded {} keysets from storage for {}",
                keysets.len(),
                mint_url
            );

            for keyset in keysets {
                cache
                    .keysets_by_id
                    .insert(keyset.id, Arc::new(keyset.clone()));

                if keyset.active {
                    cache.active_keysets.push(Arc::new(keyset));
                }
            }
        }
        Ok(_) => {
            tracing::debug!("No keysets in storage for {}", mint_url);
        }
        Err(e) => {
            tracing::warn!("Error loading keysets from storage for {}: {}", mint_url, e);
        }
    }

    // Load keys for each keyset
    for keyset_id in cache.keysets_by_id.keys() {
        match storage.get_keys(keyset_id).await {
            Ok(Some(keys)) => {
                cache.keys_by_id.insert(*keyset_id, Arc::new(keys));
            }
            Ok(None) => {
                tracing::debug!(
                    "No keys for keyset {} in storage for {}",
                    keyset_id,
                    mint_url
                );
            }
            Err(e) => {
                tracing::warn!(
                    "Error loading keys for keyset {} from storage for {}: {}",
                    keyset_id,
                    mint_url,
                    e
                );
            }
        }
    }

    // Only mark ready if we have mint_info
    cache.is_ready = cache.mint_info.is_some();

    Ok(cache)
}

/// Persist the current cache to storage
async fn write_cache_to_storage(
    mint_url: &MintUrl,
    storage: &Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync>,
    cache: &Arc<ArcSwap<MintKeyCache>>,
) {
    let cache_snapshot = cache.load();

    // Save mint info
    if let Some(mint_info) = &cache_snapshot.mint_info {
        storage
            .add_mint(mint_url.clone(), Some(mint_info.clone()))
            .await
            .inspect_err(|e| tracing::warn!("Failed to save mint info for {}: {}", mint_url, e))
            .ok();
    }

    // Save keysets (via add_mint_keysets which takes mint_url and keysets)
    let keysets: Vec<_> = cache_snapshot
        .keysets_by_id
        .values()
        .map(|ks| (**ks).clone())
        .collect();

    if !keysets.is_empty() {
        storage
            .add_mint_keysets(mint_url.clone(), keysets)
            .await
            .inspect_err(|e| tracing::warn!("Failed to save keysets for {}: {}", mint_url, e))
            .ok();
    }

    // Save keys
    for (keyset_id, keys) in &cache_snapshot.keys_by_id {
        if let Some(keyset_info) = cache_snapshot.keysets_by_id.get(keyset_id) {
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
                        "Failed to save keys for keyset {} for {}: {}",
                        keyset_id,
                        mint_url,
                        e
                    )
                })
                .ok();
        }
    }
}

/// Try to load cache from storage and update if successful
async fn try_load_cache_from_storage(
    mint_url: &MintUrl,
    storage: &Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync>,
    cache: &Arc<ArcSwap<MintKeyCache>>,
) {
    match load_cache_from_storage(mint_url, storage).await {
        Ok(loaded_cache) if loaded_cache.is_ready => {
            tracing::info!("Successfully loaded cache from storage for {}", mint_url);
            let old_version = cache.load().refresh_version;
            let mut new_cache = loaded_cache;
            new_cache.refresh_version = old_version + 1;
            cache.store(Arc::new(new_cache));
        }
        Ok(_) => {
            tracing::debug!("Storage cache for {} exists but not ready", mint_url);
        }
        Err(e) => {
            tracing::warn!("Failed to load cache from storage for {}: {}", mint_url, e);
        }
    }
}

/// Fetch fresh mint data from HTTP and update cache
///
/// Steps:
/// 1. Fetches mint info from server
/// 2. Fetches keyset list
/// 3. Fetches keys for each keyset
/// 4. Updates in-memory cache atomically
/// 5. Persists all data to storage
async fn fetch_mint_data_from_http(
    mint_url: &MintUrl,
    client: &Arc<dyn MintConnector + Send + Sync>,
    #[cfg(feature = "auth")] auth_client: &Arc<dyn AuthMintConnector + Send + Sync>,
    storage: &Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync>,
    cache: &Arc<ArcSwap<MintKeyCache>>,
) {
    tracing::debug!("Fetching mint data from HTTP for {}", mint_url);

    let mut new_cache = MintKeyCache::empty();

    // Fetch mint info
    match client.get_mint_info().await {
        Ok(mint_info) => {
            tracing::debug!("Fetched mint info for {}", mint_url);
            new_cache.mint_info = Some(mint_info);
        }
        Err(e) => {
            tracing::error!("Failed to fetch mint info for {}: {}", mint_url, e);
            return;
        }
    }

    // Fetch keysets
    let keysets = match client.get_mint_keysets().await {
        Ok(response) => response.keysets,
        Err(e) => {
            tracing::error!("Failed to fetch keysets for {}: {}", mint_url, e);
            return;
        }
    };

    #[cfg(feature = "auth")]
    let keysets = match auth_client.get_mint_blind_auth_keysets().await {
        Ok(response) => {
            let mut keysets = keysets;
            keysets.extend(response.keysets);
            keysets
        }
        Err(e) => {
            tracing::error!("Failed to fetch keysets for {}: {}", mint_url, e);
            keysets
        }
    };

    tracing::debug!("Fetched {} keysets for {}", keysets.len(), mint_url);

    // Fetch keys for each keyset
    for keyset_info in keysets {
        let keyset_arc = Arc::new(keyset_info.clone());
        new_cache
            .keysets_by_id
            .insert(keyset_info.id, keyset_arc.clone());

        if keyset_info.active {
            new_cache.active_keysets.push(keyset_arc);
        }

        // Load keys (from DB or HTTP)
        if let Ok(keyset) =
            load_keyset_from_db_or_http(mint_url, client, storage, &keyset_info.id).await
        {
            new_cache
                .keys_by_id
                .insert(keyset_info.id, Arc::new(keyset.keys));
        } else {
            tracing::warn!(
                "Failed to load keys for keyset {} for {}",
                keyset_info.id,
                mint_url
            );
        }
    }

    // Update cache atomically
    let old_version = cache.load().refresh_version;
    new_cache.is_ready = true;
    new_cache.last_refresh = std::time::Instant::now();
    new_cache.refresh_version = old_version + 1;

    tracing::info!(
        "Updating cache for {} with {} keysets (version {})",
        mint_url,
        new_cache.keysets_by_id.len(),
        new_cache.refresh_version
    );

    let cache_arc = Arc::new(new_cache);
    cache.store(cache_arc.clone());

    // Persist to storage
    write_cache_to_storage(mint_url, storage, cache).await;
}

/// Execute a single refresh task
///
/// Calls fetch_mint_data_from_http and handles any errors
async fn refresh_mint_task(
    mint_url: MintUrl,
    client: Arc<dyn MintConnector + Send + Sync>,
    #[cfg(feature = "auth")] auth_client: Arc<dyn AuthMintConnector + Send + Sync>,
    storage: Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync>,
    cache: Arc<ArcSwap<MintKeyCache>>,
) {
    fetch_mint_data_from_http(
        &mint_url,
        &client,
        #[cfg(feature = "auth")]
        &auth_client,
        &storage,
        &cache,
    )
    .await;
}

/// Background refresh loop for a single mint
///
/// Listens for messages and periodically refreshes mint data.
/// Runs until a Stop message is received.
pub(super) async fn refresh_loop(
    mint_url: MintUrl,
    client: Arc<dyn MintConnector + Send + Sync>,
    #[cfg(feature = "auth")] auth_client: Arc<dyn AuthMintConnector + Send + Sync>,
    storage: Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync>,
    cache: Arc<ArcSwap<MintKeyCache>>,
    mut rx: tokio::sync::mpsc::Receiver<MessageToWorker>,
    refresh_interval: Duration,
) {
    tracing::debug!(
        "Starting refresh loop for {} (interval: {:?})",
        mint_url,
        refresh_interval
    );

    // Try to load from storage first
    try_load_cache_from_storage(&mint_url, &storage, &cache).await;

    // Perform initial refresh from HTTP
    tracing::debug!("Performing initial HTTP refresh for {}", mint_url);
    refresh_mint_task(
        mint_url.clone(),
        client.clone(),
        #[cfg(feature = "auth")]
        auth_client.clone(),
        storage.clone(),
        cache.clone(),
    )
    .await;

    // Main event loop
    loop {
        tokio::select! {
            Some(msg) = rx.recv() => {
                match msg {
                    MessageToWorker::Stop => {
                        tracing::debug!("Stopping refresh loop for {}", mint_url);
                        break;
                    }
                    MessageToWorker::FetchMint => {
                        tracing::debug!("Manual refresh triggered for {}", mint_url);
                        refresh_mint_task(
                            mint_url.clone(),
                            client.clone(),
                            #[cfg(feature = "auth")]
                            auth_client.clone(),
                            storage.clone(),
                            cache.clone(),
                        ).await;
                    }
                }
            }
            _ = sleep(refresh_interval) => {
                tracing::debug!("Time to refresh mint: {}", mint_url);
                refresh_mint_task(
                    mint_url.clone(),
                    client.clone(),
                    #[cfg(feature = "auth")]
                    auth_client.clone(),
                    storage.clone(),
                    cache.clone(),
                ).await;
            }
        }
    }

    tracing::debug!("Refresh loop ended for {}", mint_url);
}

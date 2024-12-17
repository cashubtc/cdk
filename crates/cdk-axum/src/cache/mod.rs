//! HTTP cache.
//!
//! This is mod defines a common trait to define custom backends for the HTTP cache.
//!
//! The HTTP cache is a layer to cache responses from HTTP requests, to avoid hitting
//! the same endpoint multiple times, which can be expensive and slow, or to provide
//! idempotent operations.
//!
//! This mod also provides common backend implementations as well, such as In
//! Memory (default) and Redis.
use std::ops::Deref;
use std::sync::Arc;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Serialize;
use sha2::{Digest, Sha256};

mod backend;
mod config;

pub use self::backend::*;
pub use self::config::Config;

#[async_trait::async_trait]
/// Cache storage for the HTTP cache.
pub trait HttpCacheStorage {
    /// Sets the expiration times for the cache.
    fn set_expiration_times(&mut self, cache_ttl: Duration, cache_tti: Duration);

    /// Get a value from the cache.
    async fn get(&self, key: &HttpCacheKey) -> Option<Vec<u8>>;

    /// Set a value in the cache.
    async fn set(&self, key: HttpCacheKey, value: Vec<u8>);
}

/// Http cache with a pluggable storage backend.
pub struct HttpCache {
    /// Time to live for the cache.
    pub ttl: Duration,
    /// Time to idle for the cache.
    pub tti: Duration,
    /// Storage backend for the cache.
    storage: Arc<Box<dyn HttpCacheStorage + Send + Sync>>,
}

impl Default for HttpCache {
    fn default() -> Self {
        Self::new(
            Duration::from_secs(DEFAULT_TTL_SECS),
            Duration::from_secs(DEFAULT_TTI_SECS),
            None,
        )
    }
}

/// Max payload size for the cache key.
///
/// This is a trade-off between security and performance. A large payload can be used to
/// perform a CPU attack.
const MAX_PAYLOAD_SIZE: usize = 10 * 1024 * 1024;

/// Default TTL for the cache.
const DEFAULT_TTL_SECS: u64 = 60;

/// Default TTI for the cache.
const DEFAULT_TTI_SECS: u64 = 60;

/// Http cache key.
///
/// This type ensures no Vec<u8> is used as a key, which is error-prone.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HttpCacheKey([u8; 32]);

impl Deref for HttpCacheKey {
    type Target = [u8; 32];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<config::Config> for HttpCache {
    fn from(config: config::Config) -> Self {
        match config.backend {
            config::Backend::Memory => Self::new(
                Duration::from_secs(config.ttl.unwrap_or(DEFAULT_TTL_SECS)),
                Duration::from_secs(config.tti.unwrap_or(DEFAULT_TTI_SECS)),
                None,
            ),
            #[cfg(feature = "redis")]
            config::Backend::Redis(redis_config) => {
                let client = redis::Client::open(redis_config.connection_string)
                    .expect("Failed to create Redis client");
                let storage = HttpCacheRedis::new(client).set_prefix(
                    redis_config
                        .key_prefix
                        .unwrap_or_default()
                        .as_bytes()
                        .to_vec(),
                );
                Self::new(
                    Duration::from_secs(config.ttl.unwrap_or(DEFAULT_TTL_SECS)),
                    Duration::from_secs(config.tti.unwrap_or(DEFAULT_TTI_SECS)),
                    Some(Box::new(storage)),
                )
            }
        }
    }
}

impl HttpCache {
    /// Create a new HTTP cache.
    pub fn new(
        ttl: Duration,
        tti: Duration,
        storage: Option<Box<dyn HttpCacheStorage + Send + Sync + 'static>>,
    ) -> Self {
        let mut storage = storage.unwrap_or_else(|| Box::new(InMemoryHttpCache::default()));
        storage.set_expiration_times(ttl, tti);

        Self {
            ttl,
            tti,
            storage: Arc::new(storage),
        }
    }

    /// Calculate a cache key from a serializable value.
    ///
    /// Usually the input is the request body or query parameters.
    ///
    /// The result is an optional cache key. If the key cannot be calculated, it
    /// will be None, meaning the value cannot be cached, therefore the entire
    /// caching mechanism should be skipped.
    ///
    /// Instead of using the entire serialized input as the key, the key is a
    /// double hash to have a predictable key size, although it may open the
    /// window for CPU attacks with large payloads, but it is a trade-off.
    /// Perhaps upper layer have a protection against large payloads.
    pub fn calculate_key<K>(&self, key: &K) -> Option<HttpCacheKey>
    where
        K: Serialize,
    {
        let json_value = match serde_json::to_vec(key) {
            Ok(value) => value,
            Err(err) => {
                tracing::warn!("Failed to serialize key: {:?}", err);
                return None;
            }
        };

        if json_value.len() > MAX_PAYLOAD_SIZE {
            tracing::warn!("Key size is too large: {}", json_value.len());
            return None;
        }

        let first_hash = Sha256::digest(json_value);
        let second_hash = Sha256::digest(first_hash);
        Some(HttpCacheKey(second_hash.into()))
    }

    /// Get a value from the cache.
    pub async fn get<V>(self: &Arc<Self>, key: &HttpCacheKey) -> Option<V>
    where
        V: DeserializeOwned,
    {
        self.storage.get(key).await.and_then(|value| {
            serde_json::from_slice(&value)
                .map_err(|e| {
                    tracing::warn!("Failed to deserialize value: {:?}", e);
                    e
                })
                .ok()
        })
    }

    /// Set a value in the cache.
    pub async fn set<V: Serialize>(self: &Arc<Self>, key: HttpCacheKey, value: &V) {
        if let Ok(bytes) = serde_json::to_vec(value).map_err(|e| {
            tracing::warn!("Failed to serialize value: {:?}", e);
            e
        }) {
            self.storage.set(key, bytes).await;
        }
    }
}

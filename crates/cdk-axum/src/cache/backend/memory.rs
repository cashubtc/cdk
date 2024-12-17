use std::time::Duration;

use moka::future::Cache;

use crate::cache::{HttpCacheKey, HttpCacheStorage, DEFAULT_TTI_SECS, DEFAULT_TTL_SECS};

/// In memory cache storage for the HTTP cache.
///
/// This is the default cache storage backend, which is used if no other storage
/// backend is provided, or if the provided storage backend is `None`.
///
/// The cache is limited to 10,000 entries and it is not shared between
/// instances nor persisted.
pub struct InMemoryHttpCache(pub Cache<HttpCacheKey, Vec<u8>>);

impl Default for InMemoryHttpCache {
    fn default() -> Self {
        InMemoryHttpCache(
            Cache::builder()
                .max_capacity(10_000)
                .time_to_live(Duration::from_secs(DEFAULT_TTL_SECS))
                .time_to_idle(Duration::from_secs(DEFAULT_TTI_SECS))
                .build(),
        )
    }
}

#[async_trait::async_trait]
impl HttpCacheStorage for InMemoryHttpCache {
    fn set_expiration_times(&mut self, cache_ttl: Duration, cache_tti: Duration) {
        self.0 = Cache::builder()
            .max_capacity(10_000)
            .time_to_live(cache_ttl)
            .time_to_idle(cache_tti)
            .build();
    }

    async fn get(&self, key: &HttpCacheKey) -> Option<Vec<u8>> {
        self.0.get(key)
    }

    async fn set(&self, key: HttpCacheKey, value: Vec<u8>) {
        self.0.insert(key, value).await;
    }
}

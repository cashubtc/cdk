use std::time::Duration;

use moka::future::Cache;

use crate::cache::{HttpCacheKey, HttpCacheStorage};

/// In memory cache storage for the HTTP cache.
///
/// This is the default cache storage backend, which is used if no other storage
/// backend is provided, or if the provided storage backend is `None`.
///
/// The cache is limited to 10,000 entries and it is not shared between
/// instances nor persisted.
pub struct InMemoryHttpCache(pub Cache<HttpCacheKey, Vec<u8>>);

#[async_trait::async_trait]
impl HttpCacheStorage for InMemoryHttpCache {
    fn new(cache_ttl: Duration, cache_tti: Duration) -> Self
    where
        Self: Sized,
    {
        InMemoryHttpCache(
            Cache::builder()
                .max_capacity(10_000)
                .time_to_live(cache_ttl)
                .time_to_idle(cache_tti)
                .build(),
        )
    }

    async fn get(&self, key: &HttpCacheKey) -> Option<Vec<u8>> {
        self.0.get(key)
    }

    async fn set(&self, key: HttpCacheKey, value: Vec<u8>) {
        self.0.insert(key, value).await;
    }
}

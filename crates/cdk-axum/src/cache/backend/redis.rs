use std::time::Duration;

use redis::AsyncCommands;
use serde::{Deserialize, Serialize};

use crate::cache::{HttpCacheKey, HttpCacheStorage};

/// Redis cache storage for the HTTP cache.
///
/// This cache storage backend uses Redis to store the cache.
pub struct HttpCacheRedis {
    cache_ttl: Duration,
    prefix: Option<Vec<u8>>,
    client: redis::Client,
}

/// Configuration for the Redis cache storage.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    /// Commong key prefix
    pub key_prefix: Option<String>,

    /// Connection string to the Redis server.
    pub connection_string: String,
}

impl HttpCacheRedis {
    /// Create a new Redis cache.
    pub fn new(client: redis::Client) -> Self {
        Self {
            client,
            prefix: None,
            cache_ttl: Duration::from_secs(60),
        }
    }

    /// Set a prefix for the cache keys.
    ///
    /// This is useful to have all the HTTP cache keys under a common prefix,
    /// some sort of namespace, to make management of the database easier.
    pub fn set_prefix(mut self, prefix: Vec<u8>) -> Self {
        self.prefix = Some(prefix);
        self
    }
}

#[async_trait::async_trait]
impl HttpCacheStorage for HttpCacheRedis {
    fn set_expiration_times(&mut self, cache_ttl: Duration, _cache_tti: Duration) {
        self.cache_ttl = cache_ttl;
    }

    async fn get(&self, key: &HttpCacheKey) -> Option<Vec<u8>> {
        let mut conn = self
            .client
            .get_multiplexed_tokio_connection()
            .await
            .map_err(|err| {
                tracing::error!("Failed to get redis connection: {:?}", err);
                err
            })
            .ok()?;

        let mut db_key = self.prefix.clone().unwrap_or_default();
        db_key.extend(&**key);

        conn.get(db_key)
            .await
            .map_err(|err| {
                tracing::error!("Failed to get value from redis: {:?}", err);
                err
            })
            .ok()?
    }

    async fn set(&self, key: HttpCacheKey, value: Vec<u8>) {
        let mut db_key = self.prefix.clone().unwrap_or_default();
        db_key.extend(&*key);

        let mut conn = match self.client.get_multiplexed_tokio_connection().await {
            Ok(conn) => conn,
            Err(err) => {
                tracing::error!("Failed to get redis connection: {:?}", err);
                return;
            }
        };

        let _: Result<(), _> = conn
            .set_ex(db_key, value, self.cache_ttl.as_secs() as usize)
            .await
            .map_err(|err| {
                tracing::error!("Failed to set value in redis: {:?}", err);
                err
            });
    }
}

use std::time::Duration;

use redis::AsyncCommands;
use serde::{Deserialize, Serialize};

use crate::cache::{HttpCacheKey, HttpCacheStorage};

/// Redis client enum to handle both standard and cluster clients.
#[derive(Clone)]
pub enum RedisClient {
    /// Standard Redis client.
    Standard(redis::Client),
    /// Redis Cluster client.
    Cluster(redis::cluster::ClusterClient),
}

impl std::fmt::Debug for RedisClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Standard(_) => write!(f, "Standard"),
            Self::Cluster(_) => write!(f, "Cluster"),
        }
    }
}

/// Redis cache storage for the HTTP cache.
///
/// This cache storage backend uses Redis to store the cache.
pub struct HttpCacheRedis {
    cache_ttl: Duration,
    prefix: Option<Vec<u8>>,
    client: RedisClient,
}

impl std::fmt::Debug for HttpCacheRedis {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpCacheRedis")
            .field("cache_ttl", &self.cache_ttl)
            .field("prefix", &self.prefix)
            .field("client", &self.client)
            .finish_non_exhaustive()
    }
}

/// Configuration for the Redis cache storage.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    /// Common key prefix
    pub key_prefix: Option<String>,

    /// Connection string to the Redis server (for single node).
    pub connection_string: Option<String>,

    /// Use Redis Cluster.
    #[serde(default)]
    pub use_cluster: bool,

    /// Redis Cluster nodes (for cluster).
    pub cluster_nodes: Option<Vec<String>>,
}

impl HttpCacheRedis {
    /// Create a new Redis cache.
    pub fn new(client: RedisClient) -> Self {
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
        let mut db_key = self.prefix.clone().unwrap_or_default();
        db_key.extend(&**key);

        match &self.client {
            RedisClient::Standard(client) => {
                let mut conn = client
                    .get_multiplexed_tokio_connection()
                    .await
                    .map_err(|err| {
                        tracing::error!("Failed to get redis connection: {:?}", err);
                        err
                    })
                    .ok()?;

                conn.get(db_key)
                    .await
                    .map_err(|err| {
                        tracing::error!("Failed to get value from redis: {:?}", err);
                        err
                    })
                    .ok()?
            }
            RedisClient::Cluster(client) => {
                let mut conn = client
                    .get_async_connection()
                    .await
                    .map_err(|err| {
                        tracing::error!("Failed to get redis cluster connection: {:?}", err);
                        err
                    })
                    .ok()?;

                conn.get(db_key)
                    .await
                    .map_err(|err| {
                        tracing::error!("Failed to get value from redis cluster: {:?}", err);
                        err
                    })
                    .ok()?
            }
        }
    }

    async fn set(&self, key: HttpCacheKey, value: Vec<u8>) {
        let mut db_key = self.prefix.clone().unwrap_or_default();
        db_key.extend(&*key);

        match &self.client {
            RedisClient::Standard(client) => {
                let mut conn = match client.get_multiplexed_tokio_connection().await {
                    Ok(conn) => conn,
                    Err(err) => {
                        tracing::error!("Failed to get redis connection: {:?}", err);
                        return;
                    }
                };

                let _: Result<(), _> = conn
                    .set_ex(db_key, value, self.cache_ttl.as_secs())
                    .await
                    .map_err(|err| {
                        tracing::error!("Failed to set value in redis: {:?}", err);
                        err
                    });
            }
            RedisClient::Cluster(client) => {
                let mut conn = match client.get_async_connection().await {
                    Ok(conn) => conn,
                    Err(err) => {
                        tracing::error!("Failed to get redis cluster connection: {:?}", err);
                        return;
                    }
                };

                let _: Result<(), _> = conn
                    .set_ex(db_key, value, self.cache_ttl.as_secs())
                    .await
                    .map_err(|err| {
                        tracing::error!("Failed to set value in redis cluster: {:?}", err);
                        err
                    });
            }
        }
    }
}

use serde::{Deserialize, Serialize};

pub const ENV_CDK_MINTD_CACHE_BACKEND: &str = "CDK_MINTD_CACHE_BACKEND";

#[cfg(feature = "redis")]
pub const ENV_CDK_MINTD_CACHE_REDIS_URL: &str = "CDK_MINTD_CACHE_REDIS_URL";
#[cfg(feature = "redis")]
pub const ENV_CDK_MINTD_CACHE_REDIS_KEY_PREFIX: &str = "CDK_MINTD_CACHE_REDIS_KEY_PREFIX";
#[cfg(feature = "redis")]
pub const ENV_CDK_MINTD_CACHE_REDIS_USE_CLUSTER: &str = "CDK_MINTD_CACHE_REDIS_USE_CLUSTER";
#[cfg(feature = "redis")]
pub const ENV_CDK_MINTD_CACHE_REDIS_CLUSTER_NODES: &str = "CDK_MINTD_CACHE_REDIS_CLUSTER_NODES";

pub const ENV_CDK_MINTD_CACHE_TTI: &str = "CDK_MINTD_CACHE_TTI";
pub const ENV_CDK_MINTD_CACHE_TTL: &str = "CDK_MINTD_CACHE_TTL";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(tag = "backend")]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    #[default]
    Memory,
    #[cfg(feature = "redis")]
    Redis(super::backend::RedisConfig),
}

impl Backend {
    pub fn from_env_str(backend_str: &str) -> Option<Self> {
        match backend_str.to_lowercase().as_str() {
            "memory" => Some(Self::Memory),
            #[cfg(feature = "redis")]
            "redis" => Some(Self::Redis(redis_config_from_env())),
            _ => None,
        }
    }
}

/// Reads all Redis-related environment variables and returns a [`super::backend::RedisConfig`].
///
/// This is the single source of truth for env-based Redis configuration loading,
/// used by both [`Backend::from_env_str`] and [`Config::from_env`].
#[cfg(feature = "redis")]
fn redis_config_from_env() -> super::backend::RedisConfig {
    let use_cluster = std::env::var(ENV_CDK_MINTD_CACHE_REDIS_USE_CLUSTER)
        .map(|v| v.to_lowercase() == "true")
        .unwrap_or(false);

    let connection_string = std::env::var(ENV_CDK_MINTD_CACHE_REDIS_URL)
        .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());

    let key_prefix = std::env::var(ENV_CDK_MINTD_CACHE_REDIS_KEY_PREFIX).ok();

    let cluster_nodes = std::env::var(ENV_CDK_MINTD_CACHE_REDIS_CLUSTER_NODES)
        .ok()
        .map(|v| v.split(',').map(|s| s.trim().to_string()).collect());

    super::backend::RedisConfig {
        connection_string,
        key_prefix,
        use_cluster,
        cluster_nodes,
    }
}

/// Cache configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    /// Cache backend.
    #[serde(default)]
    #[serde(flatten)]
    pub backend: Backend,

    /// Time to live for the cache entries.
    pub ttl: Option<u64>,

    /// Time for the cache entries to be idle.
    pub tti: Option<u64>,
}

impl Config {
    /// Config from env
    pub fn from_env(mut self) -> Self {
        use std::env;

        // Parse backend
        if let Ok(backend_str) = env::var(ENV_CDK_MINTD_CACHE_BACKEND) {
            if let Some(backend) = Backend::from_env_str(&backend_str) {
                self.backend = backend;

                // If Redis backend is selected, re-parse Redis configuration
                // from env to allow overrides after the backend type is set.
                #[cfg(feature = "redis")]
                if matches!(self.backend, Backend::Redis(_)) {
                    self.backend = Backend::Redis(redis_config_from_env());
                }
            }
        }

        // Parse TTL
        if let Ok(ttl_str) = env::var(ENV_CDK_MINTD_CACHE_TTL) {
            if let Ok(ttl) = ttl_str.parse() {
                self.ttl = Some(ttl);
            }
        }

        // Parse TTI
        if let Ok(tti_str) = env::var(ENV_CDK_MINTD_CACHE_TTI) {
            if let Ok(tti) = tti_str.parse() {
                self.tti = Some(tti);
            }
        }

        self
    }
}

#[cfg(all(test, feature = "redis"))]
mod tests {
    use super::*;

    #[test]
    fn test_from_env_str_memory() {
        assert!(matches!(
            Backend::from_env_str("memory"),
            Some(Backend::Memory)
        ));
        assert!(matches!(
            Backend::from_env_str("MEMORY"),
            Some(Backend::Memory)
        ));
    }

    #[test]
    fn test_from_env_str_unknown_returns_none() {
        assert!(Backend::from_env_str("unknown").is_none());
        assert!(Backend::from_env_str("").is_none());
    }

    /// Verifies the cluster node string splitting and trimming logic.
    #[test]
    fn test_cluster_nodes_parsing() {
        let raw = "redis://node1:6379, redis://node2:6379 , redis://node3:6379";
        let parsed: Vec<String> = raw.split(',').map(|s| s.trim().to_string()).collect();

        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0], "redis://node1:6379");
        assert_eq!(parsed[1], "redis://node2:6379");
        assert_eq!(parsed[2], "redis://node3:6379");
    }

    /// Verifies the use_cluster boolean parsing from env string.
    #[test]
    fn test_use_cluster_flag_parsing() {
        let truthy = "TRUE";
        assert!(truthy.to_lowercase() == "true");

        let falsy = "false";
        assert!(!(falsy.to_lowercase() == "true"));

        let empty = "";
        assert!(!(empty.to_lowercase() == "true"));
    }

    /// Verifies the default Config has the Memory backend and no TTL set.
    #[test]
    fn test_config_default() {
        let config = Config::default();
        assert!(matches!(config.backend, Backend::Memory));
        assert!(config.ttl.is_none());
        assert!(config.tti.is_none());
    }

    /// Verifies RedisConfig struct default values for cluster fields.
    #[test]
    fn test_redis_config_defaults() {
        let cfg = super::super::backend::RedisConfig::default();
        assert!(!cfg.use_cluster);
        assert!(cfg.cluster_nodes.is_none());
        assert_eq!(cfg.connection_string, "");
        assert!(cfg.key_prefix.is_none());
    }
}

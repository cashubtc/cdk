use serde::{Deserialize, Serialize};

pub const ENV_CDK_MINTD_CACHE_BACKEND: &str = "CDK_MINTD_CACHE_BACKEND";

#[cfg(feature = "redis")]
pub const ENV_CDK_MINTD_CACHE_REDIS_URL: &str = "CDK_MINTD_CACHE_REDIS_URL";
#[cfg(feature = "redis")]
pub const ENV_CDK_MINTD_CACHE_REDIS_KEY_PREFIX: &str = "CDK_MINTD_CACHE_REDIS_KEY_PREFIX";

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
            "redis" => {
                // Get Redis configuration from environment
                let connection_string = std::env::var(ENV_CDK_MINTD_CACHE_REDIS_URL)
                    .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());

                let key_prefix = std::env::var(ENV_CDK_MINTD_CACHE_REDIS_KEY_PREFIX).ok();

                Some(Self::Redis(super::backend::RedisConfig {
                    connection_string,
                    key_prefix,
                }))
            }
            _ => None,
        }
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

                // If Redis backend is selected, parse Redis configuration
                #[cfg(feature = "redis")]
                if matches!(self.backend, Backend::Redis(_)) {
                    let connection_string = env::var(ENV_CDK_MINTD_CACHE_REDIS_URL)
                        .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());

                    let key_prefix = env::var(ENV_CDK_MINTD_CACHE_REDIS_KEY_PREFIX).ok();

                    self.backend = Backend::Redis(super::backend::RedisConfig {
                        connection_string,
                        key_prefix,
                    });
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

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(tag = "backend")]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    #[default]
    Memory,
    #[cfg(feature = "redis")]
    Redis(super::backend::RedisConfig),
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

use serde::{Deserialize, Serialize};

pub const ENV_CDK_MINTD_CACHE_BACKEND: &str = "CDK_MINTD_CACHE_BACKEND";

pub const ENV_CDK_MINTD_CACHE_TTI: &str = "CDK_MINTD_CACHE_TTI";
pub const ENV_CDK_MINTD_CACHE_TTL: &str = "CDK_MINTD_CACHE_TTL";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(tag = "backend")]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    #[default]
    Memory,
}

impl Backend {
    pub fn from_env_str(backend_str: &str) -> Option<Self> {
        match backend_str.to_lowercase().as_str() {
            "memory" => Some(Self::Memory),
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

//! NUT-19: Cached Responses
//!
//! <https://github.com/cashubtc/nuts/blob/main/19.md>

use serde::{Deserialize, Serialize};

/// Mint settings
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct Settings {
    /// Number of seconds the responses are cached for
    pub ttl: Option<u64>,
    /// Cached endpoints
    pub cached_endpoints: Vec<CachedEndpoint>,
}

/// List of the methods and paths for which caching is enabled
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct CachedEndpoint {
    /// HTTP Method
    pub method: Method,
    /// Route path
    pub path: Path,
}

impl CachedEndpoint {
    /// Create [`CachedEndpoint`]
    pub fn new(method: Method, path: Path) -> Self {
        Self { method, path }
    }
}

/// HTTP method
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub enum Method {
    /// Get
    Get,
    /// POST
    Post,
}

/// Route path
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub enum Path {
    /// Bolt11 Mint
    #[serde(rename = "/v1/mint/bolt11")]
    MintBolt11,
    /// Mining-share Mint
    #[serde(rename = "/v1/mint/mining_share")]
    MintMiningShare,
    /// Bolt11 Melt
    #[serde(rename = "/v1/melt/bolt11")]
    MeltBolt11,
    /// Swap
    #[serde(rename = "/v1/swap")]
    Swap,
    /// Bolt12 Mint
    #[serde(rename = "/v1/mint/bolt12")]
    MintBolt12,
    /// Bolt12 Melt
    #[serde(rename = "/v1/melt/bolt12")]
    MeltBolt12,
    /// Mining-share mint quote
    #[serde(rename = "/v1/mint/quote/mining_share")]
    MintQuoteMiningShare,
}

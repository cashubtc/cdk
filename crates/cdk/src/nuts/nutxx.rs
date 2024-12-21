//! XX Clear Auth

use serde::{Deserialize, Serialize};

/// List of the methods and paths that are protected
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProtectedEndpoint {
    /// HTTP Method
    pub method: Method,
    /// Route path
    pub path: RoutePath,
}

impl ProtectedEndpoint {
    /// Create [`CachedEndpoint`]
    pub fn new(method: Method, path: RoutePath) -> Self {
        Self { method, path }
    }
}

/// HTTP method
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    /// Get
    Get,
    /// POST
    Post,
}

/// Route path
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RoutePath {
    /// Bolt11 Mint Quote
    #[serde(rename = "/v1/mint/quote/bolt11")]
    MintQuoteBolt11,
    /// Bolt11 Mint
    #[serde(rename = "/v1/mint/bolt11")]
    MintBolt11,
    /// Bolt11 Melt Quote
    #[serde(rename = "/v1/melt/quote/bolt11")]
    MeltQuoteBolt11,
    /// Bolt11 Melt
    #[serde(rename = "/v1/melt/bolt11")]
    MeltBolt11,
    /// Swap
    #[serde(rename = "/v1/swap")]
    Swap,
    /// Checkstate
    #[serde(rename = "/v1/checkstate")]
    Checkstate,
    /// Restore
    #[serde(rename = "/v1/restore")]
    Restore,
}

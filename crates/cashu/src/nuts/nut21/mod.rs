//! XX Clear Auth

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// NUTXX Error
#[derive(Debug, Error)]
pub enum Error {}

/// Clear Auth Settings
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct Settings {
    /// Openid discovery
    pub openid_discovery: String,
    /// Client ID
    pub client_id: String,
    /// Protected endpoints
    pub protected_endpoints: Vec<ProtectedEndpoint>,
}

impl Settings {
    /// Create new [`Settings`]
    pub fn new(
        openid_discovery: String,
        client_id: String,
        protected_endpoints: Vec<ProtectedEndpoint>,
    ) -> Self {
        Self {
            openid_discovery,
            client_id,
            protected_endpoints,
        }
    }
}

/// List of the methods and paths that are protected
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
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
    /// Mint Blind Auth
    #[serde(rename = "/v1/auth/blind/mint")]
    MintBlindAuth,
}

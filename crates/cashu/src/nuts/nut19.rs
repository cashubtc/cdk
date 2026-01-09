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

impl Path {
    /// Create a custom mint path for a payment method
    pub fn custom_mint(method: &str) -> Self {
        Path::Custom(format!("/v1/mint/{}", method))
    }

    /// Create a custom melt path for a payment method
    pub fn custom_melt(method: &str) -> Self {
        Path::Custom(format!("/v1/melt/{}", method))
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
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub enum Path {
    /// Swap
    Swap,
    /// Custom payment method path (including bolt11, bolt12, and other methods)
    Custom(String),
}

impl Serialize for Path {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let s = match self {
            Path::Swap => "/v1/swap",
            Path::Custom(custom) => custom.as_str(),
        };
        serializer.serialize_str(s)
    }
}

impl<'de> Deserialize<'de> for Path {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(match s.as_str() {
            "/v1/swap" => Path::Swap,
            custom => Path::Custom(custom.to_string()),
        })
    }
}

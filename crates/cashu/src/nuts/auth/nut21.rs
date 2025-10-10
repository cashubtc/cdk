//! 21 Clear Auth

use std::collections::HashSet;
use std::str::FromStr;

use regex::Regex;
use serde::{Deserialize, Serialize};
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use thiserror::Error;

/// NUT21 Error
#[derive(Debug, Error)]
pub enum Error {
    /// Invalid regex pattern
    #[error("Invalid regex pattern: {0}")]
    InvalidRegex(#[from] regex::Error),
}

/// Clear Auth Settings
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize)]
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

// Custom deserializer for Settings to expand regex patterns in protected endpoints
impl<'de> Deserialize<'de> for Settings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Define a temporary struct to deserialize the raw data
        #[derive(Deserialize)]
        struct RawSettings {
            openid_discovery: String,
            client_id: String,
            protected_endpoints: Vec<RawProtectedEndpoint>,
        }

        #[derive(Deserialize)]
        struct RawProtectedEndpoint {
            method: Method,
            path: String,
        }

        // Deserialize into the temporary struct
        let raw = RawSettings::deserialize(deserializer)?;

        // Process protected endpoints, expanding regex patterns if present
        let mut protected_endpoints = HashSet::new();

        for raw_endpoint in raw.protected_endpoints {
            let expanded_paths = matching_route_paths(&raw_endpoint.path).map_err(|e| {
                serde::de::Error::custom(format!(
                    "Invalid regex pattern '{}': {}",
                    raw_endpoint.path, e
                ))
            })?;

            for path in expanded_paths {
                protected_endpoints.insert(ProtectedEndpoint::new(raw_endpoint.method, path));
            }
        }

        // Create the final Settings struct
        Ok(Settings {
            openid_discovery: raw.openid_discovery,
            client_id: raw.client_id,
            protected_endpoints: protected_endpoints.into_iter().collect(),
        })
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
    /// Create [`ProtectedEndpoint`]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, EnumIter)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RoutePath {
    /// Bolt11 Mint Quote
    #[serde(rename = "/v1/mint/quote/bolt11")]
    MintQuoteBolt11,
    /// Mining-share Mint Quote
    #[serde(rename = "/v1/mint/quote/mining_share")]
    MintQuoteMiningShare,
    /// Bolt11 Mint
    #[serde(rename = "/v1/mint/bolt11")]
    MintBolt11,
    /// Mining-share Mint
    #[serde(rename = "/v1/mint/mining_share")]
    MintMiningShare,
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
    /// Bolt12 Mint Quote
    #[serde(rename = "/v1/mint/quote/bolt12")]
    MintQuoteBolt12,
    /// Bolt12 Mint
    #[serde(rename = "/v1/mint/bolt12")]
    MintBolt12,
    /// Bolt12 Melt Quote
    #[serde(rename = "/v1/melt/quote/bolt12")]
    MeltQuoteBolt12,
    /// Bolt12 Quote
    #[serde(rename = "/v1/melt/bolt12")]
    MeltBolt12,

    /// WebSocket
    #[serde(rename = "/v1/ws")]
    Ws,
}

/// Returns [`RoutePath`]s that match regex
pub fn matching_route_paths(pattern: &str) -> Result<Vec<RoutePath>, Error> {
    let regex = Regex::from_str(pattern)?;

    Ok(RoutePath::iter()
        .filter(|path| regex.is_match(&path.to_string()))
        .collect())
}

impl std::fmt::Display for RoutePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Use serde to serialize to a JSON string, then extract the value without quotes
        let json_str = match serde_json::to_string(self) {
            Ok(s) => s,
            Err(_) => return write!(f, "<error>"),
        };
        // Remove the quotes from the JSON string
        let path = json_str.trim_matches('"');
        write!(f, "{path}")
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn test_matching_route_paths_all() {
        // Regex that matches all paths
        let paths = matching_route_paths(".*").unwrap();

        // Should match all variants
        assert_eq!(paths.len(), RoutePath::iter().count());

        // Verify all variants are included
        assert!(paths.contains(&RoutePath::MintQuoteBolt11));
        assert!(paths.contains(&RoutePath::MintBolt11));
        assert!(paths.contains(&RoutePath::MeltQuoteBolt11));
        assert!(paths.contains(&RoutePath::MeltBolt11));
        assert!(paths.contains(&RoutePath::Swap));
        assert!(paths.contains(&RoutePath::Checkstate));
        assert!(paths.contains(&RoutePath::Restore));
        assert!(paths.contains(&RoutePath::MintBlindAuth));
        assert!(paths.contains(&RoutePath::MintQuoteBolt12));
        assert!(paths.contains(&RoutePath::MintBolt12));
        assert!(paths.contains(&RoutePath::MintQuoteMiningShare));
        assert!(paths.contains(&RoutePath::MintMiningShare));
    }

    #[test]
    fn test_matching_route_paths_mint_only() {
        // Regex that matches only mint paths
        let paths = matching_route_paths("^/v1/mint/.*").unwrap();

        // Should match only mint paths
        assert_eq!(paths.len(), 6);
        assert!(paths.contains(&RoutePath::MintQuoteBolt11));
        assert!(paths.contains(&RoutePath::MintBolt11));
        assert!(paths.contains(&RoutePath::MintQuoteBolt12));
        assert!(paths.contains(&RoutePath::MintBolt12));
        assert!(paths.contains(&RoutePath::MintQuoteMiningShare));
        assert!(paths.contains(&RoutePath::MintMiningShare));

        // Should not match other paths
        assert!(!paths.contains(&RoutePath::MeltQuoteBolt11));
        assert!(!paths.contains(&RoutePath::MeltBolt11));
        assert!(!paths.contains(&RoutePath::MeltQuoteBolt12));
        assert!(!paths.contains(&RoutePath::MeltBolt12));
        assert!(!paths.contains(&RoutePath::Swap));
    }

    #[test]
    fn test_matching_route_paths_quote_only() {
        // Regex that matches only quote paths
        let paths = matching_route_paths(".*/quote/.*").unwrap();

        // Should match only quote paths
        assert_eq!(paths.len(), 5);
        assert!(paths.contains(&RoutePath::MintQuoteBolt11));
        assert!(paths.contains(&RoutePath::MeltQuoteBolt11));
        assert!(paths.contains(&RoutePath::MintQuoteBolt12));
        assert!(paths.contains(&RoutePath::MeltQuoteBolt12));
        assert!(paths.contains(&RoutePath::MintQuoteMiningShare));

        // Should not match non-quote paths
        assert!(!paths.contains(&RoutePath::MintBolt11));
        assert!(!paths.contains(&RoutePath::MeltBolt11));
    }

    #[test]
    fn test_matching_route_paths_no_match() {
        // Regex that matches nothing
        let paths = matching_route_paths("/nonexistent/path").unwrap();

        // Should match nothing
        assert!(paths.is_empty());
    }

    #[test]
    fn test_matching_route_paths_quote_bolt11_only() {
        // Regex that matches only quote paths
        let paths = matching_route_paths("/v1/mint/quote/bolt11").unwrap();

        // Should match only quote paths
        assert_eq!(paths.len(), 1);
        assert!(paths.contains(&RoutePath::MintQuoteBolt11));
    }

    #[test]
    fn test_matching_route_paths_invalid_regex() {
        // Invalid regex pattern
        let result = matching_route_paths("(unclosed parenthesis");

        // Should return an error for invalid regex
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::InvalidRegex(_)));
    }

    #[test]
    fn test_route_path_to_string() {
        // Test that to_string() returns the correct path strings
        assert_eq!(
            RoutePath::MintQuoteBolt11.to_string(),
            "/v1/mint/quote/bolt11"
        );
        assert_eq!(RoutePath::MintBolt11.to_string(), "/v1/mint/bolt11");
        assert_eq!(
            RoutePath::MeltQuoteBolt11.to_string(),
            "/v1/melt/quote/bolt11"
        );
        assert_eq!(RoutePath::MeltBolt11.to_string(), "/v1/melt/bolt11");
        assert_eq!(RoutePath::Swap.to_string(), "/v1/swap");
        assert_eq!(RoutePath::Checkstate.to_string(), "/v1/checkstate");
        assert_eq!(RoutePath::Restore.to_string(), "/v1/restore");
        assert_eq!(RoutePath::MintBlindAuth.to_string(), "/v1/auth/blind/mint");
    }

    #[test]
    fn test_settings_deserialize_direct_paths() {
        let json = r#"{
            "openid_discovery": "https://example.com/.well-known/openid-configuration",
            "client_id": "client123",
            "protected_endpoints": [
                {
                    "method": "GET",
                    "path": "/v1/mint/bolt11"
                },
                {
                    "method": "POST",
                    "path": "/v1/swap"
                }
            ]
        }"#;

        let settings: Settings = serde_json::from_str(json).unwrap();

        assert_eq!(
            settings.openid_discovery,
            "https://example.com/.well-known/openid-configuration"
        );
        assert_eq!(settings.client_id, "client123");
        assert_eq!(settings.protected_endpoints.len(), 2);

        // Check that both paths are included
        let paths = settings
            .protected_endpoints
            .iter()
            .map(|ep| (ep.method, ep.path))
            .collect::<Vec<_>>();
        assert!(paths.contains(&(Method::Get, RoutePath::MintBolt11)));
        assert!(paths.contains(&(Method::Post, RoutePath::Swap)));
    }

    #[test]
    fn test_settings_deserialize_with_regex() {
        let json = r#"{
            "openid_discovery": "https://example.com/.well-known/openid-configuration",
            "client_id": "client123",
            "protected_endpoints": [
                {
                    "method": "GET",
                    "path": "^/v1/mint/.*"
                },
                {
                    "method": "POST",
                    "path": "/v1/swap"
                }
            ]
        }"#;

        let settings: Settings = serde_json::from_str(json).unwrap();

        assert_eq!(
            settings.openid_discovery,
            "https://example.com/.well-known/openid-configuration"
        );
        assert_eq!(settings.client_id, "client123");
        assert_eq!(settings.protected_endpoints.len(), 5); // 3 mint paths + 1 swap path

        let expected_protected: HashSet<ProtectedEndpoint> = HashSet::from_iter(vec![
            ProtectedEndpoint::new(Method::Post, RoutePath::Swap),
            ProtectedEndpoint::new(Method::Get, RoutePath::MintBolt11),
            ProtectedEndpoint::new(Method::Get, RoutePath::MintQuoteBolt11),
            ProtectedEndpoint::new(Method::Get, RoutePath::MintQuoteBolt12),
            ProtectedEndpoint::new(Method::Get, RoutePath::MintBolt12),
        ]);

        let deserlized_protected = settings.protected_endpoints.into_iter().collect();

        assert_eq!(expected_protected, deserlized_protected);
    }

    #[test]
    fn test_settings_deserialize_invalid_regex() {
        let json = r#"{
            "openid_discovery": "https://example.com/.well-known/openid-configuration",
            "client_id": "client123",
            "protected_endpoints": [
                {
                    "method": "GET",
                    "path": "(unclosed parenthesis"
                }
            ]
        }"#;

        let result = serde_json::from_str::<Settings>(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_settings_deserialize_exact_path_match() {
        let json = r#"{
            "openid_discovery": "https://example.com/.well-known/openid-configuration",
            "client_id": "client123",
            "protected_endpoints": [
                {
                    "method": "GET",
                    "path": "/v1/mint/quote/bolt11"
                }
            ]
        }"#;

        let settings: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(settings.protected_endpoints.len(), 1);
        assert_eq!(settings.protected_endpoints[0].method, Method::Get);
        assert_eq!(
            settings.protected_endpoints[0].path,
            RoutePath::MintQuoteBolt11
        );
    }

    #[test]
    fn test_settings_deserialize_all_paths() {
        let json = r#"{
            "openid_discovery": "https://example.com/.well-known/openid-configuration",
            "client_id": "client123",
            "protected_endpoints": [
                {
                    "method": "GET",
                    "path": ".*"
                }
            ]
        }"#;

        let settings: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(
            settings.protected_endpoints.len(),
            RoutePath::iter().count()
        );
    }
}

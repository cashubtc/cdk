//! 21 Clear Auth

use std::collections::HashSet;
use std::str::FromStr;

use regex::Regex;
use serde::{Deserialize, Serialize};
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
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub enum RoutePath {
    /// Mint Quote for a specific payment method
    MintQuote(String),
    /// Mint for a specific payment method
    Mint(String),
    /// Melt Quote for a specific payment method
    MeltQuote(String),
    /// Melt for a specific payment method
    Melt(String),
    /// Swap
    Swap,
    /// Checkstate
    Checkstate,
    /// Restore
    Restore,
    /// Mint Blind Auth
    MintBlindAuth,
    /// WebSocket
    Ws,
}

impl Serialize for RoutePath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for RoutePath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;

        // Try to parse as a known static path first
        match s.as_str() {
            "/v1/swap" => Ok(RoutePath::Swap),
            "/v1/checkstate" => Ok(RoutePath::Checkstate),
            "/v1/restore" => Ok(RoutePath::Restore),
            "/v1/auth/blind/mint" => Ok(RoutePath::MintBlindAuth),
            "/v1/ws" => Ok(RoutePath::Ws),
            _ => {
                // Try to parse as a payment method route
                if let Some(method) = s.strip_prefix("/v1/mint/quote/") {
                    Ok(RoutePath::MintQuote(method.to_string()))
                } else if let Some(method) = s.strip_prefix("/v1/mint/") {
                    Ok(RoutePath::Mint(method.to_string()))
                } else if let Some(method) = s.strip_prefix("/v1/melt/quote/") {
                    Ok(RoutePath::MeltQuote(method.to_string()))
                } else if let Some(method) = s.strip_prefix("/v1/melt/") {
                    Ok(RoutePath::Melt(method.to_string()))
                } else {
                    // Unknown path - this might be an old database value or config
                    // Provide a helpful error message
                    Err(serde::de::Error::custom(format!(
                        "Unknown route path: {}. Valid paths are: /v1/mint/quote/{{method}}, /v1/mint/{{method}}, /v1/melt/quote/{{method}}, /v1/melt/{{method}}, /v1/swap, /v1/checkstate, /v1/restore, /v1/auth/blind/mint, /v1/ws",
                        s
                    )))
                }
            }
        }
    }
}

impl RoutePath {
    /// Get all non-payment-method route paths
    /// These are routes that don't depend on payment methods
    pub fn static_paths() -> Vec<RoutePath> {
        vec![
            RoutePath::Swap,
            RoutePath::Checkstate,
            RoutePath::Restore,
            RoutePath::MintBlindAuth,
            RoutePath::Ws,
        ]
    }

    /// Get all route paths for common payment methods (bolt11, bolt12)
    /// This is used for regex matching in configuration
    pub fn common_payment_method_paths() -> Vec<RoutePath> {
        let methods = vec!["bolt11", "bolt12"];
        let mut paths = Vec::new();

        for method in methods {
            paths.push(RoutePath::MintQuote(method.to_string()));
            paths.push(RoutePath::Mint(method.to_string()));
            paths.push(RoutePath::MeltQuote(method.to_string()));
            paths.push(RoutePath::Melt(method.to_string()));
        }

        paths
    }

    /// Get all paths for regex matching (static + common payment methods)
    pub fn all_known_paths() -> Vec<RoutePath> {
        let mut paths = Self::static_paths();
        paths.extend(Self::common_payment_method_paths());
        paths
    }
}

/// Returns [`RoutePath`]s that match regex
/// Matches against all known static paths and common payment methods (bolt11, bolt12)
pub fn matching_route_paths(pattern: &str) -> Result<Vec<RoutePath>, Error> {
    let regex = Regex::from_str(pattern)?;

    Ok(RoutePath::all_known_paths()
        .into_iter()
        .filter(|path| regex.is_match(&path.to_string()))
        .collect())
}
impl std::fmt::Display for RoutePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RoutePath::MintQuote(method) => write!(f, "/v1/mint/quote/{}", method),
            RoutePath::Mint(method) => write!(f, "/v1/mint/{}", method),
            RoutePath::MeltQuote(method) => write!(f, "/v1/melt/quote/{}", method),
            RoutePath::Melt(method) => write!(f, "/v1/melt/{}", method),
            RoutePath::Swap => write!(f, "/v1/swap"),
            RoutePath::Checkstate => write!(f, "/v1/checkstate"),
            RoutePath::Restore => write!(f, "/v1/restore"),
            RoutePath::MintBlindAuth => write!(f, "/v1/auth/blind/mint"),
            RoutePath::Ws => write!(f, "/v1/ws"),
        }
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::nut00::KnownMethod;
    use crate::PaymentMethod;

    #[test]
    fn test_matching_route_paths_all() {
        // Regex that matches all paths
        let paths = matching_route_paths(".*").unwrap();

        // Should match all known variants
        assert_eq!(paths.len(), RoutePath::all_known_paths().len());

        // Verify all variants are included
        assert!(paths.contains(&RoutePath::MintQuote(
            PaymentMethod::Known(KnownMethod::Bolt11).to_string()
        )));
        assert!(paths.contains(&RoutePath::Mint(
            PaymentMethod::Known(KnownMethod::Bolt11).to_string()
        )));
        assert!(paths.contains(&RoutePath::MeltQuote(
            PaymentMethod::Known(KnownMethod::Bolt11).to_string()
        )));
        assert!(paths.contains(&RoutePath::Melt(
            PaymentMethod::Known(KnownMethod::Bolt11).to_string()
        )));
        assert!(paths.contains(&RoutePath::MintQuote(
            PaymentMethod::Known(KnownMethod::Bolt12).to_string()
        )));
        assert!(paths.contains(&RoutePath::Mint(
            PaymentMethod::Known(KnownMethod::Bolt12).to_string()
        )));
        assert!(paths.contains(&RoutePath::Swap));
        assert!(paths.contains(&RoutePath::Checkstate));
        assert!(paths.contains(&RoutePath::Restore));
        assert!(paths.contains(&RoutePath::MintBlindAuth));
    }

    #[test]
    fn test_matching_route_paths_mint_only() {
        // Regex that matches only mint paths
        let paths = matching_route_paths("^/v1/mint/.*").unwrap();

        // Should match only mint paths (4 paths: mint quote and mint for bolt11 and bolt12)
        assert_eq!(paths.len(), 4);
        assert!(paths.contains(&RoutePath::MintQuote(
            PaymentMethod::Known(KnownMethod::Bolt11).to_string()
        )));
        assert!(paths.contains(&RoutePath::Mint(
            PaymentMethod::Known(KnownMethod::Bolt11).to_string()
        )));
        assert!(paths.contains(&RoutePath::MintQuote(
            PaymentMethod::Known(KnownMethod::Bolt12).to_string()
        )));
        assert!(paths.contains(&RoutePath::Mint(
            PaymentMethod::Known(KnownMethod::Bolt12).to_string()
        )));

        // Should not match other paths
        assert!(!paths.contains(&RoutePath::MeltQuote(
            PaymentMethod::Known(KnownMethod::Bolt11).to_string()
        )));
        assert!(!paths.contains(&RoutePath::Melt(
            PaymentMethod::Known(KnownMethod::Bolt11).to_string()
        )));
        assert!(!paths.contains(&RoutePath::MeltQuote(
            PaymentMethod::Known(KnownMethod::Bolt12).to_string()
        )));
        assert!(!paths.contains(&RoutePath::Melt(
            PaymentMethod::Known(KnownMethod::Bolt12).to_string()
        )));
        assert!(!paths.contains(&RoutePath::Swap));
    }

    #[test]
    fn test_matching_route_paths_quote_only() {
        // Regex that matches only quote paths
        let paths = matching_route_paths(".*/quote/.*").unwrap();

        // Should match only quote paths (4 paths: mint quote and melt quote for bolt11 and bolt12)
        assert_eq!(paths.len(), 4);
        assert!(paths.contains(&RoutePath::MintQuote(
            PaymentMethod::Known(KnownMethod::Bolt11).to_string()
        )));
        assert!(paths.contains(&RoutePath::MeltQuote(
            PaymentMethod::Known(KnownMethod::Bolt11).to_string()
        )));
        assert!(paths.contains(&RoutePath::MintQuote(
            PaymentMethod::Known(KnownMethod::Bolt12).to_string()
        )));
        assert!(paths.contains(&RoutePath::MeltQuote(
            PaymentMethod::Known(KnownMethod::Bolt12).to_string()
        )));

        // Should not match non-quote paths
        assert!(!paths.contains(&RoutePath::Mint(
            PaymentMethod::Known(KnownMethod::Bolt11).to_string()
        )));
        assert!(!paths.contains(&RoutePath::Melt(
            PaymentMethod::Known(KnownMethod::Bolt11).to_string()
        )));
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
        // Regex that matches only mint quote bolt11 path
        let paths = matching_route_paths("/v1/mint/quote/bolt11").unwrap();

        // Should match only this specific path
        assert_eq!(paths.len(), 1);
        assert!(paths.contains(&RoutePath::MintQuote(
            PaymentMethod::Known(KnownMethod::Bolt11).to_string()
        )));
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
            RoutePath::MintQuote(PaymentMethod::Known(KnownMethod::Bolt11).to_string()).to_string(),
            "/v1/mint/quote/bolt11"
        );
        assert_eq!(
            RoutePath::Mint(PaymentMethod::Known(KnownMethod::Bolt11).to_string()).to_string(),
            "/v1/mint/bolt11"
        );
        assert_eq!(
            RoutePath::MeltQuote(PaymentMethod::Known(KnownMethod::Bolt11).to_string()).to_string(),
            "/v1/melt/quote/bolt11"
        );
        assert_eq!(
            RoutePath::Melt(PaymentMethod::Known(KnownMethod::Bolt11).to_string()).to_string(),
            "/v1/melt/bolt11"
        );
        assert_eq!(
            RoutePath::MintQuote("paypal".to_string()).to_string(),
            "/v1/mint/quote/paypal"
        );
        assert_eq!(RoutePath::Swap.to_string(), "/v1/swap");
        assert_eq!(RoutePath::Checkstate.to_string(), "/v1/checkstate");
        assert_eq!(RoutePath::Restore.to_string(), "/v1/restore");
        assert_eq!(RoutePath::MintBlindAuth.to_string(), "/v1/auth/blind/mint");
    }

    #[test]
    fn test_route_path_serialization() {
        // Test serialization of payment method paths
        let json = serde_json::to_string(&RoutePath::Mint(
            PaymentMethod::Known(KnownMethod::Bolt11).to_string(),
        ))
        .unwrap();
        assert_eq!(json, "\"/v1/mint/bolt11\"");

        let json = serde_json::to_string(&RoutePath::MintQuote("paypal".to_string())).unwrap();
        assert_eq!(json, "\"/v1/mint/quote/paypal\"");

        // Test deserialization of payment method paths
        let path: RoutePath = serde_json::from_str("\"/v1/mint/bolt11\"").unwrap();
        assert_eq!(
            path,
            RoutePath::Mint(PaymentMethod::Known(KnownMethod::Bolt11).to_string())
        );

        let path: RoutePath = serde_json::from_str("\"/v1/melt/quote/venmo\"").unwrap();
        assert_eq!(path, RoutePath::MeltQuote("venmo".to_string()));

        // Test round-trip serialization
        let original = RoutePath::Melt(PaymentMethod::Known(KnownMethod::Bolt12).to_string());
        let json = serde_json::to_string(&original).unwrap();
        let deserialized: RoutePath = serde_json::from_str(&json).unwrap();
        assert_eq!(original, deserialized);
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
            .map(|ep| (ep.method, ep.path.clone()))
            .collect::<Vec<_>>();
        assert!(paths.contains(&(
            Method::Get,
            RoutePath::Mint(PaymentMethod::Known(KnownMethod::Bolt11).to_string())
        )));
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
        assert_eq!(settings.protected_endpoints.len(), 5); // 4 mint paths (bolt11+bolt12 quote+mint) + 1 swap path

        let expected_protected: HashSet<ProtectedEndpoint> = HashSet::from_iter(vec![
            ProtectedEndpoint::new(Method::Post, RoutePath::Swap),
            ProtectedEndpoint::new(
                Method::Get,
                RoutePath::Mint(PaymentMethod::Known(KnownMethod::Bolt11).to_string()),
            ),
            ProtectedEndpoint::new(
                Method::Get,
                RoutePath::MintQuote(PaymentMethod::Known(KnownMethod::Bolt11).to_string()),
            ),
            ProtectedEndpoint::new(
                Method::Get,
                RoutePath::MintQuote(PaymentMethod::Known(KnownMethod::Bolt12).to_string()),
            ),
            ProtectedEndpoint::new(
                Method::Get,
                RoutePath::Mint(PaymentMethod::Known(KnownMethod::Bolt12).to_string()),
            ),
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
            RoutePath::MintQuote(PaymentMethod::Known(KnownMethod::Bolt11).to_string())
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
            RoutePath::all_known_paths().len()
        );
    }
}

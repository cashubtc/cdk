//! 21 Clear Auth

use std::collections::HashSet;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// NUT21 Error
#[derive(Debug, Error)]
pub enum Error {
    /// Invalid pattern
    #[error("Invalid pattern: {0}")]
    InvalidPattern(String),
    /// Unknown route path
    #[error("Unknown route path: {0}. Valid paths are: /v1/mint/quote/{{method}}, /v1/mint/{{method}}, /v1/melt/quote/{{method}}, /v1/melt/{{method}}, /v1/swap, /v1/checkstate, /v1/restore, /v1/auth/blind/mint, /v1/ws")]
    UnknownRoute(String),
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

// Custom deserializer for Settings to expand patterns in protected endpoints
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

        // Process protected endpoints, expanding patterns if present
        let mut protected_endpoints = HashSet::new();

        for raw_endpoint in raw.protected_endpoints {
            let expanded_paths = matching_route_paths(&raw_endpoint.path).map_err(|e| {
                serde::de::Error::custom(format!("Invalid pattern '{}': {}", raw_endpoint.path, e))
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

    /// Returns how specifically this protected endpoint matches a request endpoint.
    pub fn match_specificity(&self, endpoint: &Self) -> Option<usize> {
        if self.method != endpoint.method {
            return None;
        }

        self.path.match_specificity(&endpoint.path)
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
    /// Wildcard route path prefix
    Wildcard(String),
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

impl std::str::FromStr for RoutePath {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(prefix) = s.strip_suffix('*') {
            return Self::wildcard(prefix.to_string());
        }

        // Try to parse as a known static path first
        match s {
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
                    Err(Error::UnknownRoute(s.to_string()))
                }
            }
        }
    }
}

impl<'de> Deserialize<'de> for RoutePath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        RoutePath::from_str(&s).map_err(serde::de::Error::custom)
    }
}

impl RoutePath {
    /// Create a wildcard route path.
    pub fn wildcard(prefix: String) -> Result<Self, Error> {
        if prefix.contains('*') {
            return Err(Error::InvalidPattern(
                "Wildcard '*' must be the last character".to_string(),
            ));
        }

        Ok(Self::Wildcard(prefix))
    }

    /// Returns true if this route path is a wildcard pattern.
    pub fn is_wildcard(&self) -> bool {
        matches!(self, Self::Wildcard(_))
    }

    /// Returns how specifically this route matches an endpoint.
    ///
    /// Exact routes are always preferred over wildcard routes. Wildcard routes
    /// are ranked by prefix length so `/v1/mint/quote/*` wins over `/v1/*`.
    pub fn match_specificity(&self, endpoint: &Self) -> Option<usize> {
        match self {
            Self::Wildcard(prefix) if endpoint.to_string().starts_with(prefix) => {
                Some(prefix.len())
            }
            _ if self == endpoint => Some(usize::MAX),
            _ => None,
        }
    }

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
    /// This is used for pattern matching in configuration
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

    /// Get all paths for pattern matching (static + common payment methods)
    pub fn all_known_paths() -> Vec<RoutePath> {
        let mut paths = Self::static_paths();
        paths.extend(Self::common_payment_method_paths());
        paths
    }
}

/// Returns [`RoutePath`]s that match the pattern (Exact or Prefix)
pub fn matching_route_paths(pattern: &str) -> Result<Vec<RoutePath>, Error> {
    // Check for wildcard
    if let Some(prefix) = pattern.strip_suffix('*') {
        let wildcard = RoutePath::wildcard(prefix.to_string())?;

        // Filter all known paths
        let mut paths: Vec<RoutePath> = RoutePath::all_known_paths()
            .into_iter()
            .filter(|path| path.to_string().starts_with(prefix))
            .collect();

        paths.push(wildcard);

        Ok(paths)
    } else {
        // Exact matching
        if pattern.contains('*') {
            return Err(Error::InvalidPattern(
                "Wildcard '*' must be the last character".to_string(),
            ));
        }

        match RoutePath::from_str(pattern) {
            Ok(path) => Ok(vec![path]),
            Err(e) => Err(e),
        }
    }
}
impl std::fmt::Display for RoutePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RoutePath::Wildcard(prefix) => write!(f, "{}*", prefix),
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
    fn test_matching_route_paths_root_wildcard() {
        // Pattern that matches everything
        let paths = matching_route_paths("*").unwrap();

        // Should match all known variants plus the wildcard policy.
        assert_eq!(paths.len(), RoutePath::all_known_paths().len() + 1);
        assert!(paths.contains(&RoutePath::Wildcard(String::new())));
    }

    #[test]
    fn test_matching_route_paths_middle_wildcard() {
        // Invalid wildcard position
        let result = matching_route_paths("/v1/*/mint");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::InvalidPattern(_)));
    }

    #[test]
    fn test_matching_route_paths_prefix_without_slash() {
        // "/v1/mint*" matches "/v1/mint" and "/v1/mint/..."
        let paths = matching_route_paths("/v1/mint*").unwrap();

        // Should match all mint paths + mint quote paths plus the wildcard policy.
        assert_eq!(paths.len(), 5);
        assert!(paths.contains(&RoutePath::Wildcard("/v1/mint".to_string())));

        // Should NOT match /v1/melt...
        assert!(!paths.contains(&RoutePath::MeltQuote(
            PaymentMethod::Known(KnownMethod::Bolt11).to_string()
        )));
    }

    #[test]
    fn test_matching_route_paths_exact_match_unknown() {
        let result = matching_route_paths("/v1/invalid/path");
        assert!(matches!(
            result,
            Err(Error::UnknownRoute(route)) if route == "/v1/invalid/path"
        ));
    }

    #[test]
    fn test_matching_route_paths_dynamic_method() {
        // Verify that custom payment methods are parsed correctly
        let paths = matching_route_paths("/v1/mint/custom_method").unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], RoutePath::Mint("custom_method".to_string()));
    }

    #[test]
    fn test_matching_route_paths_all() {
        // Prefix that matches all paths
        let paths = matching_route_paths("/v1/*").unwrap();

        // Should match all known variants plus the wildcard policy.
        assert_eq!(paths.len(), RoutePath::all_known_paths().len() + 1);
        assert!(paths.contains(&RoutePath::Wildcard("/v1/".to_string())));

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
        let paths = matching_route_paths("/v1/mint/*").unwrap();

        // Should match mint paths plus the wildcard policy.
        assert_eq!(paths.len(), 5);
        assert!(paths.contains(&RoutePath::Wildcard("/v1/mint/".to_string())));
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
        let paths = matching_route_paths("/v1/mint/quote/*").unwrap();

        // Should match quote paths plus the wildcard policy.
        assert_eq!(paths.len(), 3);
        assert!(paths.contains(&RoutePath::Wildcard("/v1/mint/quote/".to_string())));
        assert!(paths.contains(&RoutePath::MintQuote(
            PaymentMethod::Known(KnownMethod::Bolt11).to_string()
        )));
        assert!(paths.contains(&RoutePath::MintQuote(
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
        let result = matching_route_paths("/nonexistent/path");
        assert!(matches!(
            result,
            Err(Error::UnknownRoute(route)) if route == "/nonexistent/path"
        ));
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
        let result = matching_route_paths("/*unclosed parenthesis");

        // Should return an error for invalid regex
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::InvalidPattern(_)));
    }

    #[test]
    fn test_route_path_to_string() {
        // Test that to_string() returns the correct path strings
        assert_eq!(
            RoutePath::Wildcard("/v1/mint/".to_string()).to_string(),
            "/v1/mint/*"
        );
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
    fn test_route_path_is_wildcard() {
        assert!(RoutePath::Wildcard("/v1/mint/".to_string()).is_wildcard());
        assert!(!RoutePath::MintQuote("bolt11".to_string()).is_wildcard());
        assert!(!RoutePath::Swap.is_wildcard());
    }

    #[test]
    fn test_protected_endpoint_match_specificity() {
        let request = ProtectedEndpoint::new(
            Method::Post,
            RoutePath::MintQuote(PaymentMethod::Known(KnownMethod::Bolt11).to_string()),
        );
        let exact = ProtectedEndpoint::new(
            Method::Post,
            RoutePath::MintQuote(PaymentMethod::Known(KnownMethod::Bolt11).to_string()),
        );
        let wildcard =
            ProtectedEndpoint::new(Method::Post, RoutePath::Wildcard("/v1/mint/".to_string()));
        let different_method = ProtectedEndpoint::new(
            Method::Get,
            RoutePath::MintQuote(PaymentMethod::Known(KnownMethod::Bolt11).to_string()),
        );
        let non_matching_path = ProtectedEndpoint::new(Method::Post, RoutePath::Swap);

        assert_eq!(exact.match_specificity(&request), Some(usize::MAX));
        assert_eq!(
            wildcard.match_specificity(&request),
            Some("/v1/mint/".len())
        );
        assert_eq!(different_method.match_specificity(&request), None);
        assert_eq!(non_matching_path.match_specificity(&request), None);
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
                    "path": "/v1/mint/*"
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
        assert_eq!(settings.protected_endpoints.len(), 6); // 4 mint paths + wildcard + 1 swap path

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
            ProtectedEndpoint::new(Method::Get, RoutePath::Wildcard("/v1/mint/".to_string())),
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
                    "path": "/*wildcard_start"
                }
            ]
        }"#;

        let result = serde_json::from_str::<Settings>(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_settings_deserialize_unknown_exact_path() {
        let json = r#"{
            "openid_discovery": "https://example.com/.well-known/openid-configuration",
            "client_id": "client123",
            "protected_endpoints": [
                {
                    "method": "POST",
                    "path": "/v1/swp"
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
                    "path": "/v1/*"
                }
            ]
        }"#;

        let settings: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(
            settings.protected_endpoints.len(),
            RoutePath::all_known_paths().len() + 1
        );
    }

    #[test]
    fn test_matching_route_paths_empty_pattern() {
        let result = matching_route_paths("");
        assert!(matches!(result, Err(Error::UnknownRoute(route)) if route.is_empty()));
    }

    #[test]
    fn test_matching_route_paths_just_slash() {
        let result = matching_route_paths("/");
        assert!(matches!(result, Err(Error::UnknownRoute(route)) if route == "/"));
    }

    #[test]
    fn test_matching_route_paths_trailing_slash() {
        // Pattern with trailing slash after wildcard: "/v1/mint/*/"
        // The wildcard "*" is not the last character ("/" comes after it)
        // This should be an invalid pattern according to the spec
        let result = matching_route_paths("/v1/mint/*/");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::InvalidPattern(_)));
    }

    #[test]
    fn test_matching_route_paths_consecutive_wildcards() {
        // Pattern "**" - the first * is the suffix, second * is in prefix
        // After strip_suffix('*'), we get "*" which contains '*'
        // This should be an error because wildcard must be at the end only
        let result = matching_route_paths("**");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::InvalidPattern(_)));
    }

    #[test]
    fn test_matching_route_paths_method_specific() {
        // Test that GET and POST methods are properly distinguished
        // The matching function only returns paths, methods are handled by Settings
        // This test verifies paths are correctly matched regardless of method
        let paths = matching_route_paths("/v1/swap").unwrap();
        assert_eq!(paths.len(), 1);
        assert!(paths.contains(&RoutePath::Swap));
    }

    #[test]
    fn test_settings_mixed_methods() {
        // Test Settings with mixed methods for same path pattern
        let json = r#"{
            "openid_discovery": "https://example.com/.well-known/openid-configuration",
            "client_id": "client123",
            "protected_endpoints": [
                {
                    "method": "GET",
                    "path": "/v1/swap"
                },
                {
                    "method": "POST",
                    "path": "/v1/swap"
                }
            ]
        }"#;

        let settings: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(settings.protected_endpoints.len(), 2);

        // Check both methods are present
        let methods: Vec<_> = settings
            .protected_endpoints
            .iter()
            .map(|ep| ep.method)
            .collect();
        assert!(methods.contains(&Method::Get));
        assert!(methods.contains(&Method::Post));

        // Both should have the same path
        for ep in &settings.protected_endpoints {
            assert_eq!(ep.path, RoutePath::Swap);
        }
    }

    #[test]
    fn test_matching_route_paths_melt_prefix() {
        // Test prefix matching for melt endpoints: "/v1/melt/*"
        let paths = matching_route_paths("/v1/melt/*").unwrap();

        // Should match 4 melt paths plus the wildcard policy.
        assert_eq!(paths.len(), 5);
        assert!(paths.contains(&RoutePath::Wildcard("/v1/melt/".to_string())));
        assert!(paths.contains(&RoutePath::Melt(
            PaymentMethod::Known(KnownMethod::Bolt11).to_string()
        )));
        assert!(paths.contains(&RoutePath::MeltQuote(
            PaymentMethod::Known(KnownMethod::Bolt11).to_string()
        )));
        assert!(paths.contains(&RoutePath::Melt(
            PaymentMethod::Known(KnownMethod::Bolt12).to_string()
        )));
        assert!(paths.contains(&RoutePath::MeltQuote(
            PaymentMethod::Known(KnownMethod::Bolt12).to_string()
        )));

        // Should NOT match mint paths
        assert!(!paths.contains(&RoutePath::Mint(
            PaymentMethod::Known(KnownMethod::Bolt11).to_string()
        )));
    }

    #[test]
    fn test_matching_route_paths_static_exact() {
        // Test exact matches for static paths
        let swap_paths = matching_route_paths("/v1/swap").unwrap();
        assert_eq!(swap_paths.len(), 1);
        assert_eq!(swap_paths[0], RoutePath::Swap);

        let checkstate_paths = matching_route_paths("/v1/checkstate").unwrap();
        assert_eq!(checkstate_paths.len(), 1);
        assert_eq!(checkstate_paths[0], RoutePath::Checkstate);

        let restore_paths = matching_route_paths("/v1/restore").unwrap();
        assert_eq!(restore_paths.len(), 1);
        assert_eq!(restore_paths[0], RoutePath::Restore);

        let ws_paths = matching_route_paths("/v1/ws").unwrap();
        assert_eq!(ws_paths.len(), 1);
        assert_eq!(ws_paths[0], RoutePath::Ws);
    }

    #[test]
    fn test_matching_route_paths_auth_blind_mint() {
        // Test exact match for auth blind mint endpoint
        let paths = matching_route_paths("/v1/auth/blind/mint").unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], RoutePath::MintBlindAuth);
    }

    #[test]
    fn test_settings_empty_endpoints() {
        // Test Settings with empty protected_endpoints array
        let json = r#"{
            "openid_discovery": "https://example.com/.well-known/openid-configuration",
            "client_id": "client123",
            "protected_endpoints": []
        }"#;

        let settings: Settings = serde_json::from_str(json).unwrap();
        assert!(settings.protected_endpoints.is_empty());
    }

    #[test]
    fn test_settings_duplicate_paths() {
        // Test that duplicate paths are deduplicated by HashSet
        // Using same pattern twice with same method should result in single entry
        let json = r#"{
            "openid_discovery": "https://example.com/.well-known/openid-configuration",
            "client_id": "client123",
            "protected_endpoints": [
                {
                    "method": "POST",
                    "path": "/v1/swap"
                },
                {
                    "method": "POST",
                    "path": "/v1/swap"
                }
            ]
        }"#;

        let settings: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(settings.protected_endpoints.len(), 1);
        assert_eq!(settings.protected_endpoints[0].method, Method::Post);
        assert_eq!(settings.protected_endpoints[0].path, RoutePath::Swap);
    }

    #[test]
    fn test_matching_route_paths_only_wildcard() {
        // Pattern with just "*" matches everything
        let paths = matching_route_paths("*").unwrap();
        assert_eq!(paths.len(), RoutePath::all_known_paths().len() + 1);
        assert!(paths.contains(&RoutePath::Wildcard(String::new())));
    }

    #[test]
    fn test_matching_route_paths_wildcard_in_middle() {
        // Pattern "/v1/*/bolt11" - wildcard in the middle
        // After strip_suffix('*'), we get "/v1/*/bolt11" which contains '*'
        // This should be an error
        let result = matching_route_paths("/v1/*/bolt11");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::InvalidPattern(_)));
    }

    #[test]
    fn test_exact_match_no_child_paths() {
        // Exact match "/v1/mint" is not a valid RoutePath by itself.
        let result = matching_route_paths("/v1/mint");
        assert!(matches!(
            result,
            Err(Error::UnknownRoute(route)) if route == "/v1/mint"
        ));
    }

    #[test]
    fn test_exact_match_no_extra_path() {
        // Exact match "/v1/swap" should NOT match "/v1/swap/extra"
        // Since "/v1/swap/extra" is not a known path, it won't be in all_known_paths
        // But let's verify "/v1/swap" only matches the exact Swap path
        let paths = matching_route_paths("/v1/swap").unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], RoutePath::Swap);

        // Verify it doesn't match any other paths
        assert!(!paths.contains(&RoutePath::Checkstate));
        assert!(!paths.contains(&RoutePath::Restore));
    }

    #[test]
    fn test_partial_prefix_matching() {
        // Pattern "/v1/mi*" - partial prefix that matches "/v1/mint/..." but not "/v1/melt/..."
        let paths = matching_route_paths("/v1/mi*").unwrap();

        // This DOES match "/v1/mint/bolt11" because "/v1/mint/bolt11" starts with "/v1/mi"
        assert!(paths.contains(&RoutePath::Mint(
            PaymentMethod::Known(KnownMethod::Bolt11).to_string()
        )));
        assert!(paths.contains(&RoutePath::MintQuote(
            PaymentMethod::Known(KnownMethod::Bolt11).to_string()
        )));

        // But it does NOT match melt paths because "/v1/melt" doesn't start with "/v1/mi"
        assert!(!paths.contains(&RoutePath::Melt(
            PaymentMethod::Known(KnownMethod::Bolt11).to_string()
        )));
        assert!(!paths.contains(&RoutePath::MeltQuote(
            PaymentMethod::Known(KnownMethod::Bolt11).to_string()
        )));
    }

    #[test]
    fn test_exact_match_wrong_payment_method() {
        // Pattern "/v1/mint/quote/bolt11" should NOT match "/v1/mint/quote/bolt12"
        let paths = matching_route_paths("/v1/mint/quote/bolt11").unwrap();

        assert_eq!(paths.len(), 1);
        assert!(paths.contains(&RoutePath::MintQuote(
            PaymentMethod::Known(KnownMethod::Bolt11).to_string()
        )));

        // Should NOT contain bolt12
        assert!(!paths.contains(&RoutePath::MintQuote(
            PaymentMethod::Known(KnownMethod::Bolt12).to_string()
        )));

        // Should NOT contain regular mint (non-quote)
        assert!(!paths.contains(&RoutePath::Mint(
            PaymentMethod::Known(KnownMethod::Bolt11).to_string()
        )));
    }

    #[test]
    fn test_prefix_match_wrong_category() {
        // Pattern "/v1/mint/*" should NOT match melt paths "/v1/melt/*"
        let paths = matching_route_paths("/v1/mint/*").unwrap();

        // Should contain mint paths
        assert!(paths.contains(&RoutePath::Mint(
            PaymentMethod::Known(KnownMethod::Bolt11).to_string()
        )));
        assert!(paths.contains(&RoutePath::MintQuote(
            PaymentMethod::Known(KnownMethod::Bolt11).to_string()
        )));

        // Should NOT contain melt paths (different category)
        assert!(!paths.contains(&RoutePath::Melt(
            PaymentMethod::Known(KnownMethod::Bolt11).to_string()
        )));
        assert!(!paths.contains(&RoutePath::MeltQuote(
            PaymentMethod::Known(KnownMethod::Bolt11).to_string()
        )));

        // Should NOT contain static paths
        assert!(!paths.contains(&RoutePath::Swap));
        assert!(!paths.contains(&RoutePath::Checkstate));
    }

    #[test]
    fn test_case_sensitivity() {
        // Pattern "/v1/MINT/*" should NOT match "/v1/mint/bolt11" (case sensitive)
        let paths_upper = matching_route_paths("/v1/MINT/*").unwrap();
        let paths_lower = matching_route_paths("/v1/mint/*").unwrap();

        // Uppercase should not match any known paths, but the wildcard policy is preserved.
        assert_eq!(
            paths_upper,
            vec![RoutePath::Wildcard("/v1/MINT/".to_string())]
        );

        // Lowercase should match 4 mint paths plus the wildcard policy.
        assert_eq!(paths_lower.len(), 5);
    }

    #[test]
    fn test_negative_assertions_comprehensive() {
        // Comprehensive test that verifies multiple negative cases in one place

        // 1. Exact match for wrong payment method
        let bolt11_paths = matching_route_paths("/v1/mint/quote/bolt11").unwrap();
        assert!(!bolt11_paths.contains(&RoutePath::MintQuote(
            PaymentMethod::Known(KnownMethod::Bolt12).to_string()
        )));

        // 2. Prefix for one category doesn't match another
        let mint_paths = matching_route_paths("/v1/mint/*").unwrap();
        assert!(!mint_paths.contains(&RoutePath::Melt(
            PaymentMethod::Known(KnownMethod::Bolt11).to_string()
        )));
        assert!(!mint_paths.contains(&RoutePath::MeltQuote(
            PaymentMethod::Known(KnownMethod::Bolt11).to_string()
        )));

        // 3. Exact match for static path doesn't match others
        let swap_paths = matching_route_paths("/v1/swap").unwrap();
        assert!(!swap_paths.contains(&RoutePath::Checkstate));
        assert!(!swap_paths.contains(&RoutePath::Restore));
        assert!(!swap_paths.contains(&RoutePath::MintBlindAuth));

        // 4. Case sensitivity - wrong exact case fails, wildcard case is preserved.
        assert!(matches!(
            matching_route_paths("/V1/SWAP"),
            Err(Error::UnknownRoute(route)) if route == "/V1/SWAP"
        ));
        assert_eq!(
            matching_route_paths("/V1/MINT/*").unwrap(),
            vec![RoutePath::Wildcard("/V1/MINT/".to_string())]
        );

        // 5. Invalid/unknown exact paths fail closed.
        assert!(matches!(
            matching_route_paths("/unknown/path"),
            Err(Error::UnknownRoute(route)) if route == "/unknown/path"
        ));
        assert!(matches!(
            matching_route_paths("/invalid"),
            Err(Error::UnknownRoute(route)) if route == "/invalid"
        ));
    }

    #[test]
    fn test_prefix_vs_exact_boundary() {
        // Pattern "/v1/mint/quote/*" should NOT match "/v1/mint/quote" itself
        let paths = matching_route_paths("/v1/mint/quote/*").unwrap();

        // The pattern requires something after "/v1/mint/quote/"
        // So "/v1/mint/quote" (without payment method) is NOT a valid RoutePath
        // and won't be in the results
        assert!(!paths.is_empty()); // Should have bolt11 and bolt12

        // Verify we have the quote paths with payment methods
        assert!(paths.contains(&RoutePath::MintQuote(
            PaymentMethod::Known(KnownMethod::Bolt11).to_string()
        )));
        assert!(paths.contains(&RoutePath::MintQuote(
            PaymentMethod::Known(KnownMethod::Bolt12).to_string()
        )));

        // But there is no RoutePath::MintQuote without a payment method
        // So the list should only contain bolt11/bolt12 and the wildcard policy,
        // not a bare "/v1/mint/quote".
        assert_eq!(paths.len(), 3);
        assert!(paths.contains(&RoutePath::Wildcard("/v1/mint/quote/".to_string())));
    }

    #[test]
    fn test_wildcard_protects_custom_payment_methods() {
        let paths_star = matching_route_paths("/v1/*").unwrap();
        let paths_mint = matching_route_paths("/v1/mint/*").unwrap();

        let custom_mint = RoutePath::Mint("paypal".to_string());
        let custom_mint_quote = RoutePath::MintQuote("paypal".to_string());

        assert!(paths_star
            .iter()
            .any(|path| path.match_specificity(&custom_mint).is_some()));
        assert!(paths_star
            .iter()
            .any(|path| path.match_specificity(&custom_mint_quote).is_some()));
        assert!(paths_mint
            .iter()
            .any(|path| path.match_specificity(&custom_mint).is_some()));
        assert!(paths_mint
            .iter()
            .any(|path| path.match_specificity(&custom_mint_quote).is_some()));
    }
}

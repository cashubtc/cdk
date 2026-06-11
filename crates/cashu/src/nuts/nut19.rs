//! NUT-19: Cached Responses
//!
//! <https://github.com/cashubtc/nuts/blob/main/19.md>

use serde::{Deserialize, Serialize};

/// Mint settings
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Settings {
    /// Number of seconds the responses are cached for
    pub ttl: Option<u64>,
    /// Cached endpoints
    pub cached_endpoints: Vec<CachedEndpoint>,
}

/// List of the methods and paths for which caching is enabled
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
        Path::Custom(format!("/v1/mint/{}", encode_path_segment(method)))
    }

    /// Create a custom melt path for a payment method
    pub fn custom_melt(method: &str) -> Self {
        Path::Custom(format!("/v1/melt/{}", encode_path_segment(method)))
    }
}

fn encode_path_segment(segment: &str) -> String {
    let mut encoded = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' => {
                encoded.push(byte as char);
            }
            _ => {
                encoded.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    encoded
}

/// HTTP method
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    /// Get
    Get,
    /// POST
    Post,
}

/// Route path
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_mint_method_cannot_inject_path_segments() {
        for method in ["../../v1/swap", "..", "."] {
            let s = match Path::custom_mint(method) {
                Path::Custom(s) => s,
                other => panic!("expected Path::Custom, got {other:?}"),
            };
            let segments: Vec<&str> = s.trim_start_matches('/').split('/').collect();

            assert_eq!(
                segments.len(),
                3,
                "method injected extra path segments: {s}"
            );
            assert!(
                !segments.iter().any(|seg| *seg == ".." || *seg == "."),
                "method injected a path-traversal segment: {s}"
            );
        }
    }

    #[test]
    fn custom_melt_method_cannot_inject_path_segments() {
        for method in ["../../v1/swap", "..", "."] {
            let s = match Path::custom_melt(method) {
                Path::Custom(s) => s,
                other => panic!("expected Path::Custom, got {other:?}"),
            };
            let segments: Vec<&str> = s.trim_start_matches('/').split('/').collect();

            assert_eq!(
                segments.len(),
                3,
                "method injected extra path segments: {s}"
            );
            assert!(
                !segments.iter().any(|seg| *seg == ".." || *seg == "."),
                "method injected a path-traversal segment: {s}"
            );
        }
    }
}

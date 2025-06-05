//! 22 Blind Auth

use std::fmt;

use bitcoin::base64::engine::general_purpose;
use bitcoin::base64::Engine;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::nut21::ProtectedEndpoint;
use crate::dhke::hash_to_curve;
use crate::secret::Secret;
use crate::util::hex;
use crate::{BlindedMessage, Id, Proof, ProofDleq, PublicKey};

/// NUT22 Error
#[derive(Debug, Error)]
pub enum Error {
    /// Invalid Prefix
    #[error("Invalid prefix")]
    InvalidPrefix,
    /// Dleq proof not included
    #[error("Dleq Proof not included for auth proof")]
    DleqProofNotIncluded,
    /// Hex Error
    #[error(transparent)]
    HexError(#[from] hex::Error),
    /// Base64 error
    #[error(transparent)]
    Base64Error(#[from] bitcoin::base64::DecodeError),
    /// Serde Json error
    #[error(transparent)]
    SerdeJsonError(#[from] serde_json::Error),
    /// Utf8 parse error
    #[error(transparent)]
    Utf8ParseError(#[from] std::string::FromUtf8Error),
    /// DHKE error
    #[error(transparent)]
    DHKE(#[from] crate::dhke::Error),
}

/// Blind auth settings
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct Settings {
    /// Max number of blind auth tokens that can be minted per request
    pub bat_max_mint: u64,
    /// Protected endpoints
    pub protected_endpoints: Vec<ProtectedEndpoint>,
}

impl Settings {
    /// Create new [`Settings`]
    pub fn new(bat_max_mint: u64, protected_endpoints: Vec<ProtectedEndpoint>) -> Self {
        Self {
            bat_max_mint,
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
        use std::collections::HashSet;

        use super::nut21::matching_route_paths;

        // Define a temporary struct to deserialize the raw data
        #[derive(Deserialize)]
        struct RawSettings {
            bat_max_mint: u64,
            protected_endpoints: Vec<RawProtectedEndpoint>,
        }

        #[derive(Deserialize)]
        struct RawProtectedEndpoint {
            method: super::nut21::Method,
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
                protected_endpoints.insert(super::nut21::ProtectedEndpoint::new(
                    raw_endpoint.method,
                    path,
                ));
            }
        }

        // Create the final Settings struct
        Ok(Settings {
            bat_max_mint: raw.bat_max_mint,
            protected_endpoints: protected_endpoints.into_iter().collect(),
        })
    }
}

/// Auth Token
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthToken {
    /// Clear Auth token
    ClearAuth(String),
    /// Blind Auth token
    BlindAuth(BlindAuthToken),
}

impl fmt::Display for AuthToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ClearAuth(cat) => cat.fmt(f),
            Self::BlindAuth(bat) => bat.fmt(f),
        }
    }
}

impl AuthToken {
    /// Header key for auth token type
    pub fn header_key(&self) -> String {
        match self {
            Self::ClearAuth(_) => "Clear-auth".to_string(),
            Self::BlindAuth(_) => "Blind-auth".to_string(),
        }
    }
}

/// Required Auth
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AuthRequired {
    /// Clear Auth token
    Clear,
    /// Blind Auth token
    Blind,
}

/// Auth Proofs
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct AuthProof {
    /// `Keyset id`
    #[serde(rename = "id")]
    pub keyset_id: Id,
    /// Secret message
    #[cfg_attr(feature = "swagger", schema(value_type = String))]
    pub secret: Secret,
    /// Unblinded signature
    #[serde(rename = "C")]
    #[cfg_attr(feature = "swagger", schema(value_type = String))]
    pub c: PublicKey,
    /// Auth Proof Dleq
    pub dleq: Option<ProofDleq>,
}

impl AuthProof {
    /// Y of AuthProof
    pub fn y(&self) -> Result<PublicKey, Error> {
        Ok(hash_to_curve(self.secret.as_bytes())?)
    }
}

impl From<AuthProof> for Proof {
    fn from(value: AuthProof) -> Self {
        Self {
            amount: 1.into(),
            keyset_id: value.keyset_id,
            secret: value.secret,
            c: value.c,
            witness: None,
            dleq: value.dleq,
        }
    }
}

impl TryFrom<Proof> for AuthProof {
    type Error = Error;
    fn try_from(value: Proof) -> Result<Self, Self::Error> {
        Ok(Self {
            keyset_id: value.keyset_id,
            secret: value.secret,
            c: value.c,
            dleq: value.dleq,
        })
    }
}

/// Blind Auth Token
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlindAuthToken {
    /// [AuthProof]
    pub auth_proof: AuthProof,
}

impl BlindAuthToken {
    /// Create new [ `BlindAuthToken`]
    pub fn new(auth_proof: AuthProof) -> Self {
        BlindAuthToken { auth_proof }
    }

    /// Remove DLEQ
    ///
    /// We do not send the DLEQ to the mint as it links redemption and creation
    pub fn without_dleq(&self) -> Self {
        Self {
            auth_proof: AuthProof {
                keyset_id: self.auth_proof.keyset_id,
                secret: self.auth_proof.secret.clone(),
                c: self.auth_proof.c,
                dleq: None,
            },
        }
    }
}

impl fmt::Display for BlindAuthToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let json_string = serde_json::to_string(&self.auth_proof).map_err(|_| fmt::Error)?;
        let encoded = general_purpose::URL_SAFE.encode(json_string);
        write!(f, "authA{encoded}")
    }
}

impl std::str::FromStr for BlindAuthToken {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Check prefix and extract the base64 encoded part in one step
        let encoded = s.strip_prefix("authA").ok_or(Error::InvalidPrefix)?;

        // Decode the base64 URL-safe string
        let json_string = general_purpose::URL_SAFE.decode(encoded)?;

        // Convert bytes to UTF-8 string
        let json_str = String::from_utf8(json_string)?;

        // Deserialize the JSON string into AuthProof
        let auth_proof: AuthProof = serde_json::from_str(&json_str)?;

        Ok(BlindAuthToken { auth_proof })
    }
}

/// Mint auth request [NUT-XX]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct MintAuthRequest {
    /// Outputs
    #[cfg_attr(feature = "swagger", schema(max_items = 1_000))]
    pub outputs: Vec<BlindedMessage>,
}

impl MintAuthRequest {
    /// Count of tokens
    pub fn amount(&self) -> u64 {
        self.outputs.len() as u64
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use strum::IntoEnumIterator;

    use super::super::nut21::{Method, RoutePath};
    use super::*;

    #[test]
    fn test_settings_deserialize_direct_paths() {
        let json = r#"{
            "bat_max_mint": 10,
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

        assert_eq!(settings.bat_max_mint, 10);
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
            "bat_max_mint": 5,
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

        assert_eq!(settings.bat_max_mint, 5);
        assert_eq!(settings.protected_endpoints.len(), 5); // 4 mint paths + 1 swap path

        let expected_protected: HashSet<ProtectedEndpoint> = HashSet::from_iter(vec![
            ProtectedEndpoint::new(Method::Post, RoutePath::Swap),
            ProtectedEndpoint::new(Method::Get, RoutePath::MintBolt11),
            ProtectedEndpoint::new(Method::Get, RoutePath::MintQuoteBolt11),
            ProtectedEndpoint::new(Method::Get, RoutePath::MintQuoteBolt12),
            ProtectedEndpoint::new(Method::Get, RoutePath::MintBolt12),
        ]);

        let deserialized_protected = settings.protected_endpoints.into_iter().collect();

        assert_eq!(expected_protected, deserialized_protected);
    }

    #[test]
    fn test_settings_deserialize_invalid_regex() {
        let json = r#"{
            "bat_max_mint": 5,
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
    fn test_settings_deserialize_all_paths() {
        let json = r#"{
            "bat_max_mint": 5,
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

//! XX+1 Blind Auth

use std::fmt;

use bitcoin::base64::engine::general_purpose;
use bitcoin::base64::Engine;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::nutxx::ProtectedEndpoint;
use super::{BlindedMessage, Id, Proof, PublicKey};
use crate::dhke::hash_to_curve;
use crate::secret::Secret;
use crate::util::hex;

/// NUTxx1 Error
#[derive(Debug, Error)]
pub enum Error {
    /// Invalid Prefix
    #[error("Invalid prefix")]
    InvalidPrefix,
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
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct Settings {
    /// Max number of blind auth toeksn that can be minted per request
    pub bat_max_mint: u64,
    /// Protected endpoints
    pub protected_endpoints: Vec<ProtectedEndpoint>,
}

/// Auth Token
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AuthToken {
    /// Clear Auth token
    ClearAuth(String),
    /// Blind Auth token
    BlindAuth(BlindAuthToken),
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
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
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
            dleq: None,
        }
    }
}

/// Blind Auth Token
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlindAuthToken {
    /// [AuthProof]
    pub auth_proof: AuthProof,
}

impl fmt::Display for BlindAuthToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let json_string = serde_json::to_string(&self.auth_proof).map_err(|_| fmt::Error)?;
        let encoded = general_purpose::URL_SAFE.encode(json_string);
        write!(f, "authA{}", encoded)
    }
}

impl std::str::FromStr for BlindAuthToken {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Check if string starts with the expected prefix
        if !s.starts_with("authA") {
            return Err(Error::InvalidPrefix);
        }

        // Remove the prefix to get the base64 encoded part
        let encoded = &s["authA".len()..];

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

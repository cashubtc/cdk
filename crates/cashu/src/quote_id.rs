//! Quote ID. The specifications only define a string but CDK uses Uuid, so we use an enum to port compatibility.
use std::fmt;
use std::str::FromStr;

use bitcoin::base64::engine::general_purpose;
use bitcoin::base64::Engine as _;
use serde::{de, Deserialize, Deserializer, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// Invalid UUID
#[derive(Debug, Error)]
pub enum QuoteIdError {
    /// UUID Error
    #[error("invalid UUID: {0}")]
    Uuid(#[from] uuid::Error),
    /// Invalid base64
    #[error("invalid base64")]
    Base64,
    /// Invalid quote ID
    #[error("neither a valid UUID nor a valid base64 string")]
    InvalidQuoteId,
}

/// Mint Quote ID
#[derive(Serialize, Debug, Clone, PartialEq, PartialOrd, Ord, Eq, Hash)]
#[serde(untagged)]
pub enum QuoteId {
    /// (Nutshell) base64 quote ID
    BASE64(String),
    /// UUID quote ID
    UUID(Uuid),
}

impl QuoteId {
    /// Create a new UUID-based MintQuoteId
    pub fn new() -> Self {
        Self::UUID(Uuid::now_v7())
    }
}

impl Default for QuoteId {
    fn default() -> Self {
        Self::new()
    }
}

impl From<Uuid> for QuoteId {
    fn from(uuid: Uuid) -> Self {
        Self::UUID(uuid)
    }
}

impl fmt::Display for QuoteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QuoteId::BASE64(s) => write!(f, "{s}"),
            QuoteId::UUID(u) => write!(f, "{}", u.hyphenated()),
        }
    }
}

impl FromStr for QuoteId {
    type Err = QuoteIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Try UUID first
        if let Ok(u) = Uuid::parse_str(s) {
            return Ok(QuoteId::UUID(u));
        }

        // Try base64: decode, then re-encode and compare to ensure canonical form
        // Use the standard (URL/filename safe or standard) depending on your needed alphabet.
        // Here we use standard base64.
        match general_purpose::URL_SAFE.decode(s) {
            Ok(_bytes) => Ok(QuoteId::BASE64(s.to_string())),
            Err(_) => Err(QuoteIdError::InvalidQuoteId),
        }
    }
}

impl<'de> Deserialize<'de> for QuoteId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Deserialize as plain string first
        let s = String::deserialize(deserializer)?;

        // Try UUID first
        if let Ok(u) = Uuid::parse_str(&s) {
            return Ok(QuoteId::UUID(u));
        }

        if general_purpose::URL_SAFE.decode(&s).is_ok() {
            return Ok(QuoteId::BASE64(s));
        }

        // Neither matched — return a helpful error
        Err(de::Error::custom(format!(
            "QuoteId must be either a UUID (e.g. {}) or a valid base64 string; got: {}",
            Uuid::nil(),
            s
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quote_id_display_uuid() {
        // Test UUID display - should be hyphenated format
        let uuid = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        let quote_id = QuoteId::UUID(uuid);
        let displayed = quote_id.to_string();
        assert_eq!(displayed, "550e8400-e29b-41d4-a716-446655440000");
        assert!(!displayed.is_empty());

        // Verify roundtrip works for UUID
        let parsed: QuoteId = displayed.parse().unwrap();
        assert_eq!(quote_id, parsed);
    }

    #[test]
    fn test_quote_id_new_uses_uuid_v7() {
        let QuoteId::UUID(uuid) = QuoteId::new() else {
            panic!("new should create a UUID quote ID");
        };

        assert_eq!(uuid.get_version(), Some(uuid::Version::SortRand));
    }

    #[test]
    fn test_quote_id_from_uuid_preserves_uuid() {
        let uuid = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();

        assert_eq!(QuoteId::from(uuid), QuoteId::UUID(uuid));
    }

    #[test]
    fn test_quote_id_display_base64() {
        // Test BASE64 display - should output the string as-is
        let base64_str = "SGVsbG8gV29ybGQh"; // "Hello World!" with proper padding
        let base64_id = QuoteId::BASE64(base64_str.to_string());
        let displayed = base64_id.to_string();
        assert_eq!(displayed, base64_str);
        assert!(!displayed.is_empty());

        // Verify roundtrip works for base64
        let parsed: QuoteId = displayed.parse().unwrap();
        assert_eq!(base64_id, parsed);
    }

    #[test]
    fn test_quote_id_deserialize_uuid_preserves_uuid() {
        let quote_id: QuoteId =
            serde_json::from_str(r#""550e8400-e29b-41d4-a716-446655440000""#).unwrap();

        assert_eq!(
            quote_id,
            QuoteId::UUID(Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap())
        );
    }

    #[test]
    fn test_quote_id_deserialize_base64_preserves_base64() {
        let quote_id: QuoteId = serde_json::from_str(r#""SGVsbG8gV29ybGQh""#).unwrap();

        assert_eq!(quote_id, QuoteId::BASE64("SGVsbG8gV29ybGQh".to_string()));
    }

    #[test]
    fn test_quote_id_deserialize_rejects_invalid_id() {
        let err = serde_json::from_str::<QuoteId>(r#""not a quote id""#).unwrap_err();

        assert!(err.to_string().contains("QuoteId must be either a UUID"));
    }
}

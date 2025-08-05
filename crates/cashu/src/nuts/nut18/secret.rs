//! Secret types for NUT-18: Payment Requests
use serde::{Deserialize, Serialize};

use crate::nuts::nut10::Kind;
use crate::nuts::{Nut10Secret, SpendingConditions};

/// Nut10Secret without nonce for payment requests
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct Nut10SecretRequest {
    /// Kind of the spending condition
    #[serde(rename = "k")]
    pub kind: Kind,
    /// Secret data
    #[serde(rename = "d")]
    pub data: String,
    /// Additional data committed to and can be used for feature extensions
    #[serde(rename = "t", skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<Vec<String>>>,
}

impl Nut10SecretRequest {
    /// Create a new Nut10SecretRequest
    pub fn new<S, V>(kind: Kind, data: S, tags: Option<V>) -> Self
    where
        S: Into<String>,
        V: Into<Vec<Vec<String>>>,
    {
        Self {
            kind,
            data: data.into(),
            tags: tags.map(|v| v.into()),
        }
    }
}

impl From<Nut10Secret> for Nut10SecretRequest {
    fn from(secret: Nut10Secret) -> Self {
        Self {
            kind: secret.kind(),
            data: secret.secret_data().data().to_string(),
            tags: secret.secret_data().tags().cloned(),
        }
    }
}

impl From<Nut10SecretRequest> for Nut10Secret {
    fn from(value: Nut10SecretRequest) -> Self {
        Self::new(value.kind, value.data, value.tags)
    }
}

impl From<SpendingConditions> for Nut10SecretRequest {
    fn from(conditions: SpendingConditions) -> Self {
        match conditions {
            SpendingConditions::P2PKConditions { data, conditions } => {
                Self::new(Kind::P2PK, data.to_hex(), conditions)
            }
            SpendingConditions::HTLCConditions { data, conditions } => {
                Self::new(Kind::HTLC, data.to_string(), conditions)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nut10_secret_request_serialization() {
        let request = Nut10SecretRequest::new(
            Kind::P2PK,
            "026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198",
            Some(vec![vec!["key".to_string(), "value".to_string()]]),
        );

        let json = serde_json::to_string(&request).unwrap();

        // Verify json has abbreviated field names
        assert!(json.contains(r#""k":"P2PK""#));
        assert!(json.contains(r#""d":"026562"#));
        assert!(json.contains(r#""t":[["key","#));
    }

    #[test]
    fn test_roundtrip_serialization() {
        let original = Nut10SecretRequest {
            kind: Kind::P2PK,
            data: "test_data".into(),
            tags: Some(vec![vec!["key".to_string(), "value".to_string()]]),
        };

        let json = serde_json::to_string(&original).unwrap();
        let decoded: Nut10SecretRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(original, decoded);
    }

    #[test]
    fn test_from_nut10_secret() {
        let secret = Nut10Secret::new(
            Kind::P2PK,
            "test_data",
            Some(vec![vec!["key".to_string(), "value".to_string()]]),
        );

        let request: Nut10SecretRequest = secret.clone().into();

        assert_eq!(request.kind, secret.kind());
        assert_eq!(request.data, secret.secret_data().data());
        assert_eq!(request.tags, secret.secret_data().tags().cloned());
    }

    #[test]
    fn test_into_nut10_secret() {
        let request = Nut10SecretRequest {
            kind: Kind::HTLC,
            data: "test_hash".into(),
            tags: None,
        };

        let secret: Nut10Secret = request.clone().into();

        assert_eq!(secret.kind(), request.kind);
        assert_eq!(secret.secret_data().data(), request.data);
        assert_eq!(secret.secret_data().tags(), request.tags.as_ref());
    }
}

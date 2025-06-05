//! Secret types for NUT-18: Payment Requests

use std::fmt;

use serde::de::{self, Deserializer, SeqAccess, Visitor};
use serde::ser::{SerializeTuple, Serializer};
use serde::{Deserialize, Serialize};

use crate::nuts::nut10::Kind;
use crate::nuts::{Nut10Secret, SpendingConditions};

/// Secret Data without nonce for payment requests
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SecretDataRequest {
    /// Expresses the spending condition specific to each kind
    pub data: String,
    /// Additional data committed to and can be used for feature extensions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<Vec<String>>>,
}

/// Nut10Secret without nonce for payment requests
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct Nut10SecretRequest {
    /// Kind of the spending condition
    pub kind: Kind,
    /// Secret Data without nonce
    pub secret_data: SecretDataRequest,
}

impl Nut10SecretRequest {
    /// Create a new Nut10SecretRequest
    pub fn new<S, V>(kind: Kind, data: S, tags: Option<V>) -> Self
    where
        S: Into<String>,
        V: Into<Vec<Vec<String>>>,
    {
        let secret_data = SecretDataRequest {
            data: data.into(),
            tags: tags.map(|v| v.into()),
        };

        Self { kind, secret_data }
    }
}

impl From<Nut10Secret> for Nut10SecretRequest {
    fn from(secret: Nut10Secret) -> Self {
        let secret_data = SecretDataRequest {
            data: secret.secret_data().data().to_string(),
            tags: secret.secret_data().tags().cloned(),
        };

        Self {
            kind: secret.kind(),
            secret_data,
        }
    }
}

impl From<Nut10SecretRequest> for Nut10Secret {
    fn from(value: Nut10SecretRequest) -> Self {
        Self::new(value.kind, value.secret_data.data, value.secret_data.tags)
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

impl Serialize for Nut10SecretRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Create a tuple representing the struct fields
        let secret_tuple = (&self.kind, &self.secret_data);

        // Serialize the tuple as a JSON array
        let mut s = serializer.serialize_tuple(2)?;

        s.serialize_element(&secret_tuple.0)?;
        s.serialize_element(&secret_tuple.1)?;
        s.end()
    }
}

// Custom visitor for deserializing Secret
struct SecretVisitor;

impl<'de> Visitor<'de> for SecretVisitor {
    type Value = Nut10SecretRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a tuple with two elements: [Kind, SecretData]")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        // Deserialize the kind (first element)
        let kind = seq
            .next_element()?
            .ok_or_else(|| de::Error::invalid_length(0, &self))?;

        // Deserialize the secret_data (second element)
        let secret_data = seq
            .next_element()?
            .ok_or_else(|| de::Error::invalid_length(1, &self))?;

        // Make sure there are no additional elements
        if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
            return Err(de::Error::invalid_length(3, &self));
        }

        Ok(Nut10SecretRequest { kind, secret_data })
    }
}

impl<'de> Deserialize<'de> for Nut10SecretRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(SecretVisitor)
    }
}

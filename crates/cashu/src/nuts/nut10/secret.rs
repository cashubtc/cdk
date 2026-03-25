//! NUT-10 Secret Module

use std::fmt;
use std::str::FromStr;

use serde::de::{self, Deserializer, SeqAccess, Visitor};
use serde::ser::SerializeTuple;
use serde::{Deserialize, Serialize, Serializer};

use crate::nut10::Error;
use crate::{Kind, SecretData};

/// NUT10 Secret
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Secret {
    ///  Kind of the spending condition
    kind: Kind,
    /// Secret Data
    secret_data: SecretData,
}

impl Secret {
    /// Create new [`Secret`]
    pub fn new(kind: Kind, secret_data: SecretData) -> Self {
        Self { kind, secret_data }
    }

    /// Get the kind
    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// Get the secret data
    pub fn secret_data(&self) -> &SecretData {
        &self.secret_data
    }
}

impl Serialize for Secret {
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

impl TryFrom<Secret> for crate::secret::Secret {
    type Error = Error;
    fn try_from(secret: Secret) -> Result<crate::secret::Secret, Self::Error> {
        Ok(crate::secret::Secret::from_str(&serde_json::to_string(
            &secret,
        )?)?)
    }
}

// Custom visitor for deserializing Secret
struct SecretVisitor;

impl<'de> Visitor<'de> for SecretVisitor {
    type Value = Secret;

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

        Ok(Secret { kind, secret_data })
    }
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(SecretVisitor)
    }
}

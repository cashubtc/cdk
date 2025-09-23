//! NUT-10: Spending conditions
//!
//! <https://github.com/cashubtc/nuts/blob/main/10.md>

use std::fmt;
use std::str::FromStr;

use serde::de::{self, Deserializer, SeqAccess, Visitor};
use serde::ser::SerializeTuple;
use serde::{Deserialize, Serialize, Serializer};
use thiserror::Error;

/// NUT13 Error
#[derive(Debug, Error)]
pub enum Error {
    /// Secret error
    #[error(transparent)]
    Secret(#[from] crate::secret::Error),
    /// Serde Json error
    #[error(transparent)]
    SerdeJsonError(#[from] serde_json::Error),
}

///  NUT10 Secret Kind
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Kind {
    /// NUT-11 P2PK
    P2PK,
    /// NUT-14 HTLC
    HTLC,
}

/// Secret Date
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SecretData {
    /// Unique random string
    nonce: String,
    /// Expresses the spending condition specific to each kind
    data: String,
    /// Additional data committed to and can be used for feature extensions
    #[serde(skip_serializing_if = "Option::is_none")]
    tags: Option<Vec<Vec<String>>>,
}

impl SecretData {
    /// Create new [`SecretData`]
    pub fn new<S, V>(data: S, tags: Option<V>) -> Self
    where
        S: Into<String>,
        V: Into<Vec<Vec<String>>>,
    {
        let nonce = crate::secret::Secret::generate().to_string();

        Self {
            nonce,
            data: data.into(),
            tags: tags.map(|v| v.into()),
        }
    }

    /// Get the nonce
    pub fn nonce(&self) -> &str {
        &self.nonce
    }

    /// Get the data
    pub fn data(&self) -> &str {
        &self.data
    }

    /// Get the tags
    pub fn tags(&self) -> Option<&Vec<Vec<String>>> {
        self.tags.as_ref()
    }
}

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
    pub fn new<S, V>(kind: Kind, data: S, tags: Option<V>) -> Self
    where
        S: Into<String>,
        V: Into<Vec<Vec<String>>>,
    {
        let secret_data = SecretData::new(data, tags);
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

#[cfg(test)]
mod tests {
    use std::assert_eq;
    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_secret_serialize() {
        let secret = Secret {
            kind: Kind::P2PK,
            secret_data: SecretData {
                nonce: "5d11913ee0f92fefdc82a6764fd2457a".to_string(),
                data: "026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198"
                    .to_string(),
                tags: Some(vec![vec![
                    "key".to_string(),
                    "value1".to_string(),
                    "value2".to_string(),
                ]]),
            },
        };

        let secret_str = r#"["P2PK",{"nonce":"5d11913ee0f92fefdc82a6764fd2457a","data":"026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198","tags":[["key","value1","value2"]]}]"#;

        assert_eq!(serde_json::to_string(&secret).unwrap(), secret_str);
    }

    #[test]
    fn test_secret_round_trip_serialization() {
        // Create a Secret instance
        let original_secret = Secret {
            kind: Kind::P2PK,
            secret_data: SecretData {
                nonce: "5d11913ee0f92fefdc82a6764fd2457a".to_string(),
                data: "026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198"
                    .to_string(),
                tags: None,
            },
        };

        // Serialize the Secret to JSON string
        let serialized = serde_json::to_string(&original_secret).unwrap();

        // Deserialize directly back to Secret using serde
        let deserialized_secret: Secret = serde_json::from_str(&serialized).unwrap();

        // Verify the direct serde serialization/deserialization round trip works
        assert_eq!(original_secret, deserialized_secret);

        // Also verify that the conversion to crate::secret::Secret works
        let cashu_secret = crate::secret::Secret::from_str(&serialized).unwrap();
        let deserialized_from_cashu: Secret = TryFrom::try_from(&cashu_secret).unwrap();
        assert_eq!(original_secret, deserialized_from_cashu);
    }

    #[test]
    fn test_htlc_secret_round_trip() {
        // The reference BOLT11 invoice is:
        // lnbc100n1p5z3a63pp56854ytysg7e5z9fl3w5mgvrlqjfcytnjv8ff5hm5qt6gl6alxesqdqqcqzzsxqyz5vqsp5p0x0dlhn27s63j4emxnk26p7f94u0lyarnfp5yqmac9gzy4ngdss9qxpqysgqne3v0hnzt2lp0hc69xpzckk0cdcar7glvjhq60lsrfe8gejdm8c564prrnsft6ctxxyrewp4jtezrq3gxxqnfjj0f9tw2qs9y0lslmqpfu7et9

        // Payment hash (typical 32 byte hash in hex format)
        let payment_hash = "5c23fc3aec9d985bd5fc88ca8bceaccc52cf892715dd94b42b84f1b43350751e";

        // Create a Secret instance with HTLC kind
        let original_secret = Secret {
            kind: Kind::HTLC,
            secret_data: SecretData {
                nonce: "7a9128b3f9612549f9278958337a5d7f".to_string(),
                data: payment_hash.to_string(),
                tags: None,
            },
        };

        // Serialize the Secret to JSON string
        let serialized = serde_json::to_string(&original_secret).unwrap();

        // Validate serialized format
        let expected_json = format!(
            r#"["HTLC",{{"nonce":"7a9128b3f9612549f9278958337a5d7f","data":"{}"}}]"#,
            payment_hash
        );
        assert_eq!(serialized, expected_json);

        // Deserialize directly back to Secret using serde
        let deserialized_secret: Secret = serde_json::from_str(&serialized).unwrap();

        // Verify the direct serde serialization/deserialization round trip works
        assert_eq!(original_secret, deserialized_secret);
        assert_eq!(deserialized_secret.kind, Kind::HTLC);
        assert_eq!(deserialized_secret.secret_data.data, payment_hash);
    }
}

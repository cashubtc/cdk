//! Transport types for NUT-18: Payment Requests

use std::fmt;
use std::str::FromStr;

use bitcoin::base64::engine::{general_purpose, GeneralPurpose};
use bitcoin::base64::{alphabet, Engine};
use serde::{Deserialize, Deserializer, Serialize};

use crate::nuts::nut18::error::Error;

/// Transport Type
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransportType {
    /// Nostr
    #[serde(rename = "nostr")]
    Nostr,
    /// Http post
    #[serde(rename = "post")]
    HttpPost,
}

impl fmt::Display for TransportType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use serde::ser::Error;
        let t = serde_json::to_string(self).map_err(|e| fmt::Error::custom(e.to_string()))?;
        write!(f, "{t}")
    }
}

impl FromStr for TransportType {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "nostr" => Ok(Self::Nostr),
            "post" => Ok(Self::HttpPost),
            _ => Err(Error::InvalidPrefix),
        }
    }
}

/// Transport
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transport {
    /// Type
    #[serde(rename = "t")]
    pub _type: TransportType,
    /// Target
    #[serde(rename = "a")]
    pub target: String,
    /// Tags
    #[serde(rename = "g")]
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        default,
        deserialize_with = "deserialize_tags"
    )]
    pub tags: Vec<Vec<String>>,
}

fn deserialize_tags<'de, D>(deserializer: D) -> Result<Vec<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<Vec<Vec<String>>>::deserialize(deserializer)?.unwrap_or_default())
}

impl Transport {
    /// Create a new TransportBuilder
    pub fn builder() -> TransportBuilder {
        TransportBuilder::default()
    }
}

impl FromStr for Transport {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let decode_config = general_purpose::GeneralPurposeConfig::new()
            .with_decode_padding_mode(bitcoin::base64::engine::DecodePaddingMode::Indifferent);
        let decoded = GeneralPurpose::new(&alphabet::URL_SAFE, decode_config).decode(s)?;

        Ok(ciborium::from_reader(&decoded[..])?)
    }
}

/// Builder for Transport
#[derive(Debug, Default, Clone)]
pub struct TransportBuilder {
    _type: Option<TransportType>,
    target: Option<String>,
    tags: Vec<Vec<String>>,
}

impl TransportBuilder {
    /// Set transport type
    pub fn transport_type(mut self, transport_type: TransportType) -> Self {
        self._type = Some(transport_type);
        self
    }

    /// Set target
    pub fn target<S: Into<String>>(mut self, target: S) -> Self {
        self.target = Some(target.into());
        self
    }

    /// Add a tag
    pub fn add_tag(mut self, tag: Vec<String>) -> Self {
        self.tags.push(tag);
        self
    }

    /// Set tags
    pub fn tags(mut self, tags: Vec<Vec<String>>) -> Self {
        self.tags = tags;
        self
    }

    /// Build the Transport
    pub fn build(self) -> Result<Transport, &'static str> {
        let _type = self._type.ok_or("Transport type is required")?;
        let target = self.target.ok_or("Target is required")?;

        Ok(Transport {
            _type,
            target,
            tags: self.tags,
        })
    }
}

impl AsRef<String> for Transport {
    fn as_ref(&self) -> &String {
        &self.target
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_type_display_and_from_str_cover_wire_values() {
        assert_eq!(TransportType::Nostr.to_string(), "\"nostr\"");
        assert_eq!(TransportType::HttpPost.to_string(), "\"post\"");
        assert_eq!(
            TransportType::from_str("nostr").unwrap(),
            TransportType::Nostr
        );
        assert_eq!(
            TransportType::from_str("post").unwrap(),
            TransportType::HttpPost
        );
    }

    #[test]
    fn builder_preserves_target_and_tags() {
        let tags = vec![vec!["n".to_string(), "17".to_string()]];
        let transport = Transport::builder()
            .transport_type(TransportType::Nostr)
            .target("wss://relay.example.com")
            .tags(tags.clone())
            .build()
            .unwrap();

        assert_eq!(transport._type, TransportType::Nostr);
        assert_eq!(transport.as_ref(), "wss://relay.example.com");
        assert_eq!(transport.tags, tags);
    }
}

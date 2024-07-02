//! NUT-06: Mint Information
//!
//! <https://github.com/cashubtc/nuts/blob/main/06.md>

use std::fmt;

use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::nut01::PublicKey;
use super::{nut04, nut05, nut15};

/// Mint Version
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MintVersion {
    /// Mint Software name
    pub name: String,
    /// Mint Version
    pub version: String,
}

impl Serialize for MintVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let combined = format!("{}/{}", self.name, self.version);
        serializer.serialize_str(&combined)
    }
}

impl<'de> Deserialize<'de> for MintVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let combined = String::deserialize(deserializer)?;
        let parts: Vec<&str> = combined.split('/').collect();
        if parts.len() != 2 {
            return Err(serde::de::Error::custom("Invalid input string"));
        }
        Ok(MintVersion {
            name: parts[0].to_string(),
            version: parts[1].to_string(),
        })
    }
}

/// Mint Info [NIP-09]
#[derive(Default, Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MintInfo {
    /// name of the mint and should be recognizable
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// hex pubkey of the mint
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pubkey: Option<PublicKey>,
    /// implementation name and the version running
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<MintVersion>,
    /// short description of the mint
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// long description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description_long: Option<String>,
    /// Contact info
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(deserialize_with = "deserialize_contact_info")]
    pub contact: Option<Vec<ContactInfo>>,
    /// shows which NUTs the mint supports
    pub nuts: Nuts,
    /// message of the day that the wallet must display to the user
    #[serde(skip_serializing_if = "Option::is_none")]
    pub motd: Option<String>,
}

/// Supported nuts and settings
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Nuts {
    /// NUT04 Settings
    #[serde(default)]
    #[serde(rename = "4")]
    pub nut04: nut04::Settings,
    /// NUT05 Settings
    #[serde(default)]
    #[serde(rename = "5")]
    pub nut05: nut05::Settings,
    /// NUT07 Settings
    #[serde(default)]
    #[serde(rename = "7")]
    pub nut07: SupportedSettings,
    /// NUT08 Settings
    #[serde(default)]
    #[serde(rename = "8")]
    pub nut08: SupportedSettings,
    /// NUT09 Settings
    #[serde(default)]
    #[serde(rename = "9")]
    pub nut09: SupportedSettings,
    /// NUT10 Settings
    #[serde(rename = "10")]
    #[serde(default)]
    pub nut10: SupportedSettings,
    /// NUT11 Settings
    #[serde(rename = "11")]
    #[serde(default)]
    pub nut11: SupportedSettings,
    /// NUT12 Settings
    #[serde(default)]
    #[serde(rename = "12")]
    pub nut12: SupportedSettings,
    /// NUT13 Settings
    #[serde(default)]
    #[serde(rename = "13")]
    pub nut13: SupportedSettings,
    /// NUT14 Settings
    #[serde(default)]
    #[serde(rename = "14")]
    pub nut14: SupportedSettings,
    /// NUT15 Settings
    #[serde(default)]
    #[serde(rename = "15")]
    pub nut15: nut15::MppMethodSettings,
}

/// Check state Settings
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SupportedSettings {
    supported: bool,
}

impl Default for SupportedSettings {
    fn default() -> Self {
        Self { supported: true }
    }
}

/// Contact Info
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContactInfo {
    /// Contact Method i.e. nostr
    pub method: String,
    /// Contact info i.e. npub...
    pub info: String,
}

fn deserialize_contact_info<'de, D>(deserializer: D) -> Result<Option<Vec<ContactInfo>>, D::Error>
where
    D: Deserializer<'de>,
{
    struct ContactInfoVisitor;

    impl<'de> Visitor<'de> for ContactInfoVisitor {
        type Value = Option<Vec<ContactInfo>>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a list of ContactInfo or a list of lists of strings")
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let mut contacts = Vec::new();

            while let Some(value) = seq.next_element::<serde_json::Value>()? {
                if value.is_object() {
                    // Deserialize as ContactInfo
                    let contact: ContactInfo =
                        serde_json::from_value(value).map_err(de::Error::custom)?;
                    contacts.push(contact);
                } else if value.is_array() {
                    // Deserialize as Vec<String>
                    let vec = value
                        .as_array()
                        .ok_or_else(|| de::Error::custom("expected a list of strings"))?;
                    println!("{:?}", vec[0]);
                    for val in vec {
                        let vec = val
                            .as_array()
                            .ok_or_else(|| de::Error::custom("expected a list of strings"))?;
                        if vec.len() == 2 {
                            let method = vec[0]
                                .as_str()
                                .ok_or_else(|| de::Error::custom("expected a string"))?
                                .to_string();
                            let info = vec[1]
                                .as_str()
                                .ok_or_else(|| de::Error::custom("expected a string"))?
                                .to_string();
                            contacts.push(ContactInfo { method, info });
                        } else {
                            return Err(de::Error::custom("expected a list of two strings"));
                        }
                    }
                } else {
                    return Err(de::Error::custom("expected an object or a list of strings"));
                }
            }

            Ok(Some(contacts))
        }
    }

    deserializer.deserialize_seq(ContactInfoVisitor)
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn test_ser_mint_info() {
        /*
                let mint_info = serde_json::to_string(&MintInfo {
                    name: Some("Cashu-crab".to_string()),
                    pubkey: None,
                    version: None,
                    description: Some("A mint".to_string()),
                    description_long: Some("Some longer test".to_string()),
                    contact: None,
                    nuts: Nuts::default(),
                    motd: None,
                })
                .unwrap();

                println!("{}", mint_info);
        */
        let mint_info_str = r#"{
  "name": "Bob's Cashu mint",
  "pubkey": "0283bf290884eed3a7ca2663fc0260de2e2064d6b355ea13f98dec004b7a7ead99",
  "version": "Nutshell/0.15.0",
  "description": "The short mint description",
  "description_long": "A description that can be a long piece of text.",
  "contact": [
    {
        "method": "nostr",
        "info": "xxxxx"
    },
    {
        "method": "email",
        "info": "contact@me.com"
    }
  ],
  "motd": "Message to display to users.",
  "nuts": {
    "4": {
      "methods": [
        {
        "method": "bolt11",
        "unit": "sat",
        "min_amount": 0,
        "max_amount": 10000
        }
      ],
      "disabled": false
    },
    "5": {
      "methods": [
        {
        "method": "bolt11",
        "unit": "sat",
        "min_amount": 0,
        "max_amount": 10000
        }
      ],
      "disabled": false
    },
    "7": {"supported": true},
    "8": {"supported": true},
    "9": {"supported": true},
    "10": {"supported": true},
    "12": {"supported": true}
  }
}"#;
        let info: MintInfo = serde_json::from_str(mint_info_str).unwrap();
        let mint_info_str = r#"{
  "name": "Bob's Cashu mint",
  "pubkey": "0283bf290884eed3a7ca2663fc0260de2e2064d6b355ea13f98dec004b7a7ead99",
  "version": "Nutshell/0.15.0",
  "description": "The short mint description",
  "description_long": "A description that can be a long piece of text.",
  "contact": [
    [
        ["nostr", "xxxxx"],
        ["email", "contact@me.com"]
    ]
  ],
  "motd": "Message to display to users.",
  "nuts": {
    "4": {
      "methods": [
        {
        "method": "bolt11",
        "unit": "sat",
        "min_amount": 0,
        "max_amount": 10000
        }
      ],
      "disabled": false
    },
    "5": {
      "methods": [
        {
        "method": "bolt11",
        "unit": "sat",
        "min_amount": 0,
        "max_amount": 10000
        }
      ],
      "disabled": false
    },
    "7": {"supported": true},
    "8": {"supported": true},
    "9": {"supported": true},
    "10": {"supported": true},
    "12": {"supported": true}
  }
}"#;
        let mint_info: MintInfo = serde_json::from_str(mint_info_str).unwrap();

        assert_eq!(info, mint_info);
    }
}

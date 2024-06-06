//! NUT-06: Mint Information
//!
//! <https://github.com/cashubtc/nuts/blob/main/06.md>

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::nut01::PublicKey;
use super::{nut04, nut05, nut15};

/// Mint Version
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MintVersion {
    pub name: String,
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
    /// contact methods to reach the mint operator
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contact: Option<Vec<Vec<String>>>,
    /// shows which NUTs the mint supports
    pub nuts: Nuts,
    /// message of the day that the wallet must display to the user
    #[serde(skip_serializing_if = "Option::is_none")]
    pub motd: Option<String>,
}

/// Supported nuts and settings
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Nuts {
    #[serde(default)]
    #[serde(rename = "4")]
    pub nut04: nut04::Settings,
    #[serde(default)]
    #[serde(rename = "5")]
    pub nut05: nut05::Settings,
    #[serde(default)]
    #[serde(rename = "7")]
    pub nut07: SupportedSettings,
    #[serde(default)]
    #[serde(rename = "8")]
    pub nut08: SupportedSettings,
    #[serde(default)]
    #[serde(rename = "9")]
    pub nut09: SupportedSettings,
    #[serde(rename = "10")]
    #[serde(default)]
    pub nut10: SupportedSettings,
    #[serde(rename = "11")]
    #[serde(default)]
    pub nut11: SupportedSettings,
    #[serde(default)]
    #[serde(rename = "12")]
    pub nut12: SupportedSettings,
    #[serde(default)]
    #[serde(rename = "13")]
    pub nut13: SupportedSettings,
    #[serde(default)]
    #[serde(rename = "14")]
    pub nut14: SupportedSettings,
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
    ["email", "contact@me.com"],
    ["twitter", "@me"],
    ["nostr" ,"npub..."]
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
        let _info: MintInfo = serde_json::from_str(mint_info_str).unwrap();
    }
}

//! NUT-06: Mint Information
//!
//! <https://github.com/cashubtc/nuts/blob/main/06.md>

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::nut01::PublicKey;
use super::nut17::SupportedMethods;
use super::nut19::CachedEndpoint;
use super::{nut04, nut05, nut15, nut19, MppMethodSettings};

/// Mint Version
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct MintVersion {
    /// Mint Software name
    pub name: String,
    /// Mint Version
    pub version: String,
}

impl MintVersion {
    /// Create new [`MintVersion`]
    pub fn new(name: String, version: String) -> Self {
        Self { name, version }
    }
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

/// Mint Info [NIP-06]
#[derive(Default, Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
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
    pub contact: Option<Vec<ContactInfo>>,
    /// shows which NUTs the mint supports
    pub nuts: Nuts,
    /// Mint's icon URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
    /// Mint's endpoint URLs
    #[serde(skip_serializing_if = "Option::is_none")]
    pub urls: Option<Vec<String>>,
    /// message of the day that the wallet must display to the user
    #[serde(skip_serializing_if = "Option::is_none")]
    pub motd: Option<String>,
    /// server unix timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<u64>,
}

impl MintInfo {
    /// Create new [`MintInfo`]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set name
    pub fn name<S>(self, name: S) -> Self
    where
        S: Into<String>,
    {
        Self {
            name: Some(name.into()),
            ..self
        }
    }

    /// Set pubkey
    pub fn pubkey(self, pubkey: PublicKey) -> Self {
        Self {
            pubkey: Some(pubkey),
            ..self
        }
    }

    /// Set [`MintVersion`]
    pub fn version(self, mint_version: MintVersion) -> Self {
        Self {
            version: Some(mint_version),
            ..self
        }
    }

    /// Set description
    pub fn description<S>(self, description: S) -> Self
    where
        S: Into<String>,
    {
        Self {
            description: Some(description.into()),
            ..self
        }
    }

    /// Set long description
    pub fn long_description<S>(self, description_long: S) -> Self
    where
        S: Into<String>,
    {
        Self {
            description_long: Some(description_long.into()),
            ..self
        }
    }

    /// Set contact info
    pub fn contact_info(self, contact_info: Vec<ContactInfo>) -> Self {
        Self {
            contact: Some(contact_info),
            ..self
        }
    }

    /// Set nuts
    pub fn nuts(self, nuts: Nuts) -> Self {
        Self { nuts, ..self }
    }

    /// Set mint icon url
    pub fn icon_url<S>(self, icon_url: S) -> Self
    where
        S: Into<String>,
    {
        Self {
            icon_url: Some(icon_url.into()),
            ..self
        }
    }

    /// Set motd
    pub fn motd<S>(self, motd: S) -> Self
    where
        S: Into<String>,
    {
        Self {
            motd: Some(motd.into()),
            ..self
        }
    }

    /// Set time
    pub fn time<S>(self, time: S) -> Self
    where
        S: Into<u64>,
    {
        Self {
            time: Some(time.into()),
            ..self
        }
    }
}

/// Supported nuts and settings
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
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
    /// NUT14 Settings
    #[serde(default)]
    #[serde(rename = "14")]
    pub nut14: SupportedSettings,
    /// NUT15 Settings
    #[serde(default)]
    #[serde(rename = "15")]
    pub nut15: nut15::Settings,
    /// NUT17 Settings
    #[serde(default)]
    #[serde(rename = "17")]
    pub nut17: super::nut17::SupportedSettings,
    /// NUT19 Settings
    #[serde(default)]
    #[serde(rename = "19")]
    pub nut19: nut19::Settings,
    /// NUT20 Settings
    #[serde(default)]
    #[serde(rename = "20")]
    pub nut20: SupportedSettings,
}

impl Nuts {
    /// Create new [`Nuts`]
    pub fn new() -> Self {
        Self::default()
    }

    /// Nut04 settings
    pub fn nut04(self, nut04_settings: nut04::Settings) -> Self {
        Self {
            nut04: nut04_settings,
            ..self
        }
    }

    /// Nut05 settings
    pub fn nut05(self, nut05_settings: nut05::Settings) -> Self {
        Self {
            nut05: nut05_settings,
            ..self
        }
    }

    /// Nut07 settings
    pub fn nut07(self, supported: bool) -> Self {
        Self {
            nut07: SupportedSettings { supported },
            ..self
        }
    }

    /// Nut08 settings
    pub fn nut08(self, supported: bool) -> Self {
        Self {
            nut08: SupportedSettings { supported },
            ..self
        }
    }

    /// Nut09 settings
    pub fn nut09(self, supported: bool) -> Self {
        Self {
            nut09: SupportedSettings { supported },
            ..self
        }
    }

    /// Nut10 settings
    pub fn nut10(self, supported: bool) -> Self {
        Self {
            nut10: SupportedSettings { supported },
            ..self
        }
    }

    /// Nut11 settings
    pub fn nut11(self, supported: bool) -> Self {
        Self {
            nut11: SupportedSettings { supported },
            ..self
        }
    }

    /// Nut12 settings
    pub fn nut12(self, supported: bool) -> Self {
        Self {
            nut12: SupportedSettings { supported },
            ..self
        }
    }

    /// Nut14 settings
    pub fn nut14(self, supported: bool) -> Self {
        Self {
            nut14: SupportedSettings { supported },
            ..self
        }
    }

    /// Nut15 settings
    pub fn nut15(self, mpp_settings: Vec<MppMethodSettings>) -> Self {
        Self {
            nut15: nut15::Settings {
                methods: mpp_settings,
            },
            ..self
        }
    }

    /// Nut17 settings
    pub fn nut17(self, supported: Vec<SupportedMethods>) -> Self {
        Self {
            nut17: super::nut17::SupportedSettings { supported },
            ..self
        }
    }

    /// Nut19 settings
    pub fn nut19(self, ttl: Option<u64>, cached_endpoints: Vec<CachedEndpoint>) -> Self {
        Self {
            nut19: nut19::Settings {
                ttl,
                cached_endpoints,
            },
            ..self
        }
    }

    /// Nut20 settings
    pub fn nut20(self, supported: bool) -> Self {
        Self {
            nut20: SupportedSettings { supported },
            ..self
        }
    }
}

/// Check state Settings
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct SupportedSettings {
    supported: bool,
}

/// Contact Info
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct ContactInfo {
    /// Contact Method i.e. nostr
    pub method: String,
    /// Contact info i.e. npub...
    pub info: String,
}

impl ContactInfo {
    /// Create new [`ContactInfo`]
    pub fn new(method: String, info: String) -> Self {
        Self { method, info }
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn test_des_mint_into() {
        let mint_info_str = r#"{
    "name": "Cashu mint",
    "pubkey": "0296d0aa13b6a31cf0cd974249f28c7b7176d7274712c95a41c7d8066d3f29d679",
    "version": "Nutshell/0.15.3",
    "contact": [
        ["", ""],
        ["", ""]
    ],
    "nuts": {
        "4": {
            "methods": [
                {"method": "bolt11", "unit": "sat", "description": true},
                {"method": "bolt11", "unit": "usd", "description": true}
            ],
            "disabled": false
        },
        "5": {
            "methods": [
                {"method": "bolt11", "unit": "sat"},
                {"method": "bolt11", "unit": "usd"}
            ],
            "disabled": false
        },
        "7": {"supported": true},
        "8": {"supported": true},
        "9": {"supported": true},
        "10": {"supported": true},
        "11": {"supported": true}
    }
}"#;

        let _mint_info: MintInfo = serde_json::from_str(mint_info_str).unwrap();
    }

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
  "icon_url": "https://this-is-a-mint-icon-url.com/icon.png",
  "nuts": {
    "4": {
      "methods": [
        {
        "method": "bolt11",
        "unit": "sat",
        "min_amount": 0,
        "max_amount": 10000,
        "description": true
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
        ["nostr", "xxxxx"],
        ["email", "contact@me.com"]
  ],
  "motd": "Message to display to users.",
  "icon_url": "https://this-is-a-mint-icon-url.com/icon.png",
  "nuts": {
    "4": {
      "methods": [
        {
        "method": "bolt11",
        "unit": "sat",
        "min_amount": 0,
        "max_amount": 10000,
        "description": true
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

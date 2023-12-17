//! Mint Information
// https://github.com/cashubtc/nuts/blob/main/09.md

use std::collections::HashMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use super::nut01::PublicKey;
use super::{nut04, nut05};

/// Mint Version
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    #[serde(deserialize_with = "deserialize_nuts")]
    pub nuts: HashMap<u8, NutSettings>,
    /// message of the day that the wallet must display to the user
    #[serde(skip_serializing_if = "Option::is_none")]
    pub motd: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NutSettings {
    Nut04(nut04::Settings),
    Nut05(nut05::Settings),
    Optional(OptionalSettings),
    UnknownNut(Value),
}

fn deserialize_nuts<'de, D>(deserializer: D) -> Result<HashMap<u8, NutSettings>, D::Error>
where
    D: Deserializer<'de>,
{
    let b: Map<_, _> = Deserialize::deserialize(deserializer).unwrap();

    let h: HashMap<u8, Value> = b
        .into_iter()
        .map(|(v, k)| (v.parse().unwrap(), k))
        .collect();

    let mut nuts: HashMap<u8, NutSettings> = HashMap::with_capacity(h.capacity());

    for (num, nut) in h {
        let nut_settings = match num {
            4 => {
                let settings: nut04::Settings = serde_json::from_value(nut).unwrap();

                NutSettings::Nut04(settings)
            }
            5 => {
                let settings: nut05::Settings = serde_json::from_value(nut).unwrap();

                NutSettings::Nut05(settings)
            }
            7..=10 | 12 => {
                println!("{}", nut);
                let settings: OptionalSettings = serde_json::from_value(nut).unwrap();

                NutSettings::Optional(settings)
            }
            _ => {
                let settings: Value = serde_json::from_value(nut).unwrap();

                NutSettings::UnknownNut(settings)
            }
        };
        nuts.insert(num, nut_settings);
    }

    Ok(nuts)
}

/// Spendable Settings
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OptionalSettings {
    supported: bool,
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn test_mint_info() {
        let mint_info = r#"{"name":"moksha-mint","pubkey":"02b3d8d8280b26f1223dc603a9b2a69618dc26821bef8ee22d419c44d710007cbc","version":"0.1.2","description":"mutiny signet mint v1 api","contact":[["[[email"],["ngutech21@pm.me]]"]],"nuts":{"4":{"methods":[["bolt11","sat"]],"disabled":false},"5":{"methods":[["bolt11","sat"]]},"6":{"supported":true},"7":{"supported":false},"8":{"supported":true},"9":{"supported":false},"10":{"supported":false},"11":{"supported":false},"12":{"supported":false}}}"#;

        let _info: MintInfo = serde_json::from_str(mint_info).unwrap();
    }
}

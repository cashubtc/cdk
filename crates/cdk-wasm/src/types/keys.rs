//! Key-related WASM types

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::amount::CurrencyUnit;
use crate::error::WasmError;

/// WASM-compatible KeySetInfo
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeySetInfo {
    pub id: String,
    pub unit: CurrencyUnit,
    pub active: bool,
    /// Input fee per thousand (ppk)
    pub input_fee_ppk: u64,
}

impl From<cdk::nuts::KeySetInfo> for KeySetInfo {
    fn from(keyset: cdk::nuts::KeySetInfo) -> Self {
        Self {
            id: keyset.id.to_string(),
            unit: keyset.unit.into(),
            active: keyset.active,
            input_fee_ppk: keyset.input_fee_ppk,
        }
    }
}

impl From<KeySetInfo> for cdk::nuts::KeySetInfo {
    fn from(keyset: KeySetInfo) -> Self {
        use std::str::FromStr;
        Self {
            id: cdk::nuts::Id::from_str(&keyset.id).expect("Invalid keyset ID"),
            unit: keyset.unit.into(),
            active: keyset.active,
            final_expiry: None,
            input_fee_ppk: keyset.input_fee_ppk,
        }
    }
}

impl KeySetInfo {
    /// Convert KeySetInfo to JSON string
    pub fn to_json(&self) -> Result<String, WasmError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode KeySetInfo from JSON string
pub fn decode_key_set_info(json: String) -> Result<KeySetInfo, WasmError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode KeySetInfo to JSON string
pub fn encode_key_set_info(info: KeySetInfo) -> Result<String, WasmError> {
    Ok(serde_json::to_string(&info)?)
}

/// WASM-compatible PublicKey
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PublicKey {
    /// Hex-encoded public key
    pub hex: String,
}

impl From<cdk::nuts::PublicKey> for PublicKey {
    fn from(key: cdk::nuts::PublicKey) -> Self {
        Self {
            hex: key.to_string(),
        }
    }
}

impl TryFrom<PublicKey> for cdk::nuts::PublicKey {
    type Error = WasmError;

    fn try_from(key: PublicKey) -> Result<Self, Self::Error> {
        key.hex
            .parse()
            .map_err(|e| WasmError::internal(format!("Invalid public key: {}", e)))
    }
}

/// WASM-compatible Keys (simplified - contains only essential info)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Keys {
    /// Keyset ID
    pub id: String,
    /// Currency unit
    pub unit: CurrencyUnit,
    /// Map of amount to public key hex (simplified from BTreeMap)
    pub keys: HashMap<u64, String>,
}

impl From<cdk::nuts::Keys> for Keys {
    fn from(keys: cdk::nuts::Keys) -> Self {
        // Keys doesn't have id and unit - we'll need to get these from context
        // For now, use placeholder values
        Self {
            id: "unknown".to_string(), // This should come from KeySet
            unit: CurrencyUnit::Sat,   // This should come from KeySet
            keys: keys
                .keys()
                .iter()
                .map(|(amount, pubkey)| (u64::from(*amount), pubkey.to_string()))
                .collect(),
        }
    }
}

impl TryFrom<Keys> for cdk::nuts::Keys {
    type Error = WasmError;

    fn try_from(keys: Keys) -> Result<Self, Self::Error> {
        use std::collections::BTreeMap;
        use std::str::FromStr;

        // Convert the HashMap to BTreeMap with proper types
        let mut keys_map = BTreeMap::new();
        for (amount_u64, pubkey_hex) in keys.keys {
            let amount = cdk::Amount::from(amount_u64);
            let pubkey = cdk::nuts::PublicKey::from_str(&pubkey_hex)
                .map_err(|e| WasmError::internal(format!("Invalid public key: {}", e)))?;
            keys_map.insert(amount, pubkey);
        }

        Ok(cdk::nuts::Keys::new(keys_map))
    }
}

impl Keys {
    /// Convert Keys to JSON string
    pub fn to_json(&self) -> Result<String, WasmError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode Keys from JSON string
pub fn decode_keys(json: String) -> Result<Keys, WasmError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode Keys to JSON string
pub fn encode_keys(keys: Keys) -> Result<String, WasmError> {
    Ok(serde_json::to_string(&keys)?)
}

/// WASM-compatible KeySet
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeySet {
    /// Keyset ID
    pub id: String,
    /// Currency unit
    pub unit: CurrencyUnit,
    /// Keyset state - indicates whether the mint will sign new outputs with this keyset
    pub active: Option<bool>,
    /// Input fee in parts per thousand (ppk) per input spent from this keyset
    pub input_fee_ppk: u64,
    /// The keys (map of amount to public key hex)
    pub keys: HashMap<u64, String>,
    /// Optional expiry timestamp
    pub final_expiry: Option<u64>,
}

impl From<cdk::nuts::KeySet> for KeySet {
    fn from(keyset: cdk::nuts::KeySet) -> Self {
        Self {
            id: keyset.id.to_string(),
            unit: keyset.unit.into(),
            active: keyset.active,
            input_fee_ppk: keyset.input_fee_ppk,
            keys: keyset
                .keys
                .keys()
                .iter()
                .map(|(amount, pubkey)| (u64::from(*amount), pubkey.to_string()))
                .collect(),
            final_expiry: keyset.final_expiry,
        }
    }
}

impl TryFrom<KeySet> for cdk::nuts::KeySet {
    type Error = WasmError;

    fn try_from(keyset: KeySet) -> Result<Self, Self::Error> {
        use std::collections::BTreeMap;
        use std::str::FromStr;

        // Convert id
        let id = cdk::nuts::Id::from_str(&keyset.id)
            .map_err(|e| WasmError::internal(format!("Invalid keyset ID: {}", e)))?;

        // Convert unit
        let unit: cdk::nuts::CurrencyUnit = keyset.unit.into();

        // Convert keys
        let mut keys_map = BTreeMap::new();
        for (amount_u64, pubkey_hex) in keyset.keys {
            let amount = cdk::Amount::from(amount_u64);
            let pubkey = cdk::nuts::PublicKey::from_str(&pubkey_hex)
                .map_err(|e| WasmError::internal(format!("Invalid public key: {}", e)))?;
            keys_map.insert(amount, pubkey);
        }
        let keys = cdk::nuts::Keys::new(keys_map);

        Ok(cdk::nuts::KeySet {
            id,
            unit,
            active: keyset.active,
            input_fee_ppk: keyset.input_fee_ppk,
            keys,
            final_expiry: keyset.final_expiry,
        })
    }
}

impl KeySet {
    /// Convert KeySet to JSON string
    pub fn to_json(&self) -> Result<String, WasmError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode KeySet from JSON string
pub fn decode_key_set(json: String) -> Result<KeySet, WasmError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode KeySet to JSON string
pub fn encode_key_set(keyset: KeySet) -> Result<String, WasmError> {
    Ok(serde_json::to_string(&keyset)?)
}

/// WASM-compatible Id (for keyset IDs)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Id {
    pub hex: String,
}

impl From<cdk::nuts::Id> for Id {
    fn from(id: cdk::nuts::Id) -> Self {
        Self {
            hex: id.to_string(),
        }
    }
}

impl From<Id> for cdk::nuts::Id {
    fn from(id: Id) -> Self {
        use std::str::FromStr;
        Self::from_str(&id.hex).expect("Invalid ID hex")
    }
}

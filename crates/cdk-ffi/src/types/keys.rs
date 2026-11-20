//! Key-related FFI types

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::amount::CurrencyUnit;
use super::codec::decode_json;
use crate::error::FfiError;

/// FFI-compatible KeySetInfo
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
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

impl TryFrom<KeySetInfo> for cdk::nuts::KeySetInfo {
    type Error = FfiError;

    fn try_from(keyset: KeySetInfo) -> Result<Self, Self::Error> {
        use std::str::FromStr;

        Ok(Self {
            id: cdk::nuts::Id::from_str(&keyset.id)
                .map_err(|e| FfiError::internal(format!("Invalid keyset ID: {}", e)))?,
            unit: keyset.unit.into(),
            active: keyset.active,
            final_expiry: None,
            input_fee_ppk: keyset.input_fee_ppk,
        })
    }
}

impl KeySetInfo {
    /// Convert KeySetInfo to NUT-02 JSON
    pub fn to_json(&self) -> Result<String, FfiError> {
        encode_key_set_info(self.clone())
    }
}

/// Decode KeySetInfo from JSON string
///
/// The JSON is NUT-02's, so it goes through the native type rather than this
/// record's derived serde, whose enum representation spells a unit `Sat` where
/// a mint sends `sat`. A record an earlier version of this crate encoded is
/// still accepted, so a host that persisted one can load it.
#[uniffi::export]
pub fn decode_key_set_info(json: String) -> Result<KeySetInfo, FfiError> {
    decode_json::<cdk::nuts::KeySetInfo, _>(&json)
}

/// Encode KeySetInfo to JSON string
///
/// Produces NUT-02's encoding, so the result is what a mint would send and
/// what [`decode_key_set_info`] accepts.
#[uniffi::export]
pub fn encode_key_set_info(info: KeySetInfo) -> Result<String, FfiError> {
    let info: cdk::nuts::KeySetInfo = info.try_into()?;
    Ok(serde_json::to_string(&info)?)
}

/// FFI-compatible PublicKey
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
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
    type Error = FfiError;

    fn try_from(key: PublicKey) -> Result<Self, Self::Error> {
        key.hex
            .parse()
            .map_err(|e| FfiError::internal(format!("Invalid public key: {}", e)))
    }
}

/// FFI-compatible Keys
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct Keys {
    /// Map of amount to public key hex
    pub keys: HashMap<u64, String>,
}

impl From<cdk::nuts::Keys> for Keys {
    fn from(keys: cdk::nuts::Keys) -> Self {
        Self {
            keys: keys
                .keys()
                .iter()
                .map(|(amount, pubkey)| (u64::from(*amount), pubkey.to_string()))
                .collect(),
        }
    }
}

impl TryFrom<Keys> for cdk::nuts::Keys {
    type Error = FfiError;

    fn try_from(keys: Keys) -> Result<Self, Self::Error> {
        use std::collections::BTreeMap;
        use std::str::FromStr;

        // Convert the HashMap to BTreeMap with proper types
        let mut keys_map = BTreeMap::new();
        for (amount_u64, pubkey_hex) in keys.keys {
            let amount = cdk::Amount::from(amount_u64);
            let pubkey = cdk::nuts::PublicKey::from_str(&pubkey_hex)
                .map_err(|e| FfiError::internal(format!("Invalid public key: {}", e)))?;
            keys_map.insert(amount, pubkey);
        }

        Ok(cdk::nuts::Keys::new(keys_map))
    }
}

impl Keys {
    /// Convert Keys to JSON string
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode Keys from JSON string
#[uniffi::export]
pub fn decode_keys(json: String) -> Result<Keys, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode Keys to JSON string
#[uniffi::export]
pub fn encode_keys(keys: Keys) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&keys)?)
}

/// FFI-compatible KeySet
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
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
            keys: Keys::from(keyset.keys).keys,
            final_expiry: keyset.final_expiry,
        }
    }
}

impl TryFrom<KeySet> for cdk::nuts::KeySet {
    type Error = FfiError;

    fn try_from(keyset: KeySet) -> Result<Self, Self::Error> {
        use std::str::FromStr;

        let id = cdk::nuts::Id::from_str(&keyset.id)
            .map_err(|e| FfiError::internal(format!("Invalid keyset ID: {}", e)))?;
        let unit: cdk::nuts::CurrencyUnit = keyset.unit.into();
        let keys = Keys { keys: keyset.keys }.try_into()?;

        Ok(Self {
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
    /// Convert KeySet to NUT-02 JSON
    pub fn to_json(&self) -> Result<String, FfiError> {
        encode_key_set(self.clone())
    }
}

/// Decode KeySet from JSON string
///
/// The JSON is NUT-02's, so it goes through the native type rather than this
/// record's derived serde, which spells a unit `Sat` where a mint sends `sat`
/// and refuses a keyset that omits `input_fee_ppk`. A record an earlier version
/// of this crate encoded is still accepted, so a host that persisted one can
/// load it.
#[uniffi::export]
pub fn decode_key_set(json: String) -> Result<KeySet, FfiError> {
    decode_json::<cdk::nuts::KeySet, _>(&json)
}

/// Encode KeySet to JSON string
///
/// Produces NUT-02's encoding, so the result is an entry of what a mint sends
/// from `/v1/keys` and what [`decode_key_set`] accepts.
#[uniffi::export]
pub fn encode_key_set(keyset: KeySet) -> Result<String, FfiError> {
    let keyset: cdk::nuts::KeySet = keyset.try_into()?;
    Ok(serde_json::to_string(&keyset)?)
}

/// FFI-compatible Id (for keyset IDs)
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
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

impl TryFrom<Id> for cdk::nuts::Id {
    type Error = FfiError;

    fn try_from(id: Id) -> Result<Self, Self::Error> {
        use std::str::FromStr;

        Self::from_str(&id.hex).map_err(|e| FfiError::internal(format!("Invalid ID hex: {}", e)))
    }
}

#[cfg(test)]
mod tests {
    use std::iter::once;

    use cdk::nuts::{
        CurrencyUnit as CdkCurrencyUnit, KeySet as CdkKeySet, KeySetInfo as CdkKeySetInfo,
        Keys as CdkKeys, SecretKey,
    };
    use cdk::Amount;

    use super::*;

    const KEYSET_ID: &str = "00916bbf7ef91a36";
    const PUBKEY: &str = "03a40f20667ed53513075dc51e715ff2046cad64eb68960632269ba7f0210e38bc";

    fn native_key_set_info() -> CdkKeySetInfo {
        CdkKeySetInfo {
            id: KEYSET_ID.parse().unwrap(),
            unit: CdkCurrencyUnit::Sat,
            active: true,
            input_fee_ppk: 100,
            final_expiry: None,
        }
    }

    fn native_key_set(final_expiry: Option<u64>) -> CdkKeySet {
        let key = SecretKey::generate().public_key();
        CdkKeySet {
            id: KEYSET_ID.parse().unwrap(),
            unit: CdkCurrencyUnit::Sat,
            active: Some(true),
            keys: CdkKeys::new(once((Amount::from(1), key)).collect()),
            input_fee_ppk: 100,
            final_expiry,
        }
    }

    /// The exported codecs are what a host points at a mint's response, so they
    /// have to speak NUT-02 rather than these records' derived serde.
    #[test]
    fn key_set_info_json_is_the_mints_encoding() {
        let expected = native_key_set_info();

        let json = encode_key_set_info(expected.clone().into()).unwrap();
        assert_eq!(json, serde_json::to_string(&expected).unwrap());

        let decoded: CdkKeySetInfo = decode_key_set_info(json).unwrap().try_into().unwrap();
        assert_eq!(decoded, expected);
    }

    #[test]
    fn key_set_json_is_the_mints_encoding() {
        for final_expiry in [Some(1_900_000_000), None] {
            let expected = native_key_set(final_expiry);

            let json = encode_key_set(expected.clone().into()).unwrap();
            assert_eq!(json, serde_json::to_string(&expected).unwrap());

            let decoded: CdkKeySet = decode_key_set(json).unwrap().try_into().unwrap();
            assert_eq!(decoded, expected);
        }
    }

    /// NUT-02 reads an absent input_fee_ppk as zero. The record's derived
    /// serde rejected such a keyset outright.
    #[test]
    fn an_absent_input_fee_decodes_to_zero() {
        let mut json = serde_json::to_value(native_key_set(None)).unwrap();
        json.as_object_mut().unwrap().remove("input_fee_ppk");
        let keyset = decode_key_set(json.to_string()).unwrap();
        assert_eq!(keyset.input_fee_ppk, 0);
    }

    /// A host that persisted what an earlier version of this crate encoded has
    /// a custom unit spelled as the FFI enum.
    #[test]
    fn a_record_encoded_before_the_codecs_moved_still_decodes() {
        let info = decode_key_set_info(format!(
            r#"{{"id":"{KEYSET_ID}","unit":{{"Custom":{{"unit":"usdt"}}}},"active":true,"input_fee_ppk":100}}"#
        ))
        .unwrap();
        assert_eq!(
            info.unit,
            CurrencyUnit::Custom {
                unit: "usdt".to_string()
            }
        );

        let native: CdkKeySetInfo = info.try_into().unwrap();
        assert_eq!(native.unit, CdkCurrencyUnit::custom("usdt"));

        let keyset = decode_key_set(format!(
            r#"{{"id":"{KEYSET_ID}","unit":{{"Custom":{{"unit":"usdt"}}}},"active":true,"input_fee_ppk":100,"keys":{{"1":"{PUBKEY}"}},"final_expiry":null}}"#
        ))
        .unwrap();
        assert_eq!(
            keyset.unit,
            CurrencyUnit::Custom {
                unit: "usdt".to_string()
            }
        );
        assert_eq!(keyset.final_expiry, None);

        let native: CdkKeySet = keyset.try_into().unwrap();
        assert_eq!(native.unit, CdkCurrencyUnit::custom("usdt"));
    }

    /// Only a custom unit needed the fallback: the enum's spelling of a known
    /// unit is one NUT-00 already reads, since it parses case-insensitively.
    #[test]
    fn a_legacy_known_unit_decodes_through_the_protocol_path() {
        let info = decode_key_set_info(format!(
            r#"{{"id":"{KEYSET_ID}","unit":"Sat","active":true,"input_fee_ppk":100}}"#
        ))
        .unwrap();
        assert_eq!(info.unit, CurrencyUnit::Sat);
    }

    #[test]
    fn json_in_neither_format_is_rejected() {
        let error = decode_key_set_info(format!(
            r#"{{"id":"{KEYSET_ID}","unit":{{"Bogus":{{"unit":"usdt"}}}},"active":true}}"#
        ))
        .unwrap_err()
        .to_string();
        assert!(error.contains("protocol encoding"), "{error}");
        assert!(error.contains("this crate encoded"), "{error}");
    }

    #[test]
    fn to_json_matches_the_exported_encoder() {
        let info: KeySetInfo = native_key_set_info().into();
        assert_eq!(
            info.to_json().unwrap(),
            encode_key_set_info(info.clone()).unwrap()
        );

        let keyset: KeySet = native_key_set(Some(1_900_000_000)).into();
        assert_eq!(
            keyset.to_json().unwrap(),
            encode_key_set(keyset.clone()).unwrap()
        );
        assert_eq!(
            decode_key_set(keyset.to_json().unwrap())
                .unwrap()
                .final_expiry,
            keyset.final_expiry
        );
    }
}

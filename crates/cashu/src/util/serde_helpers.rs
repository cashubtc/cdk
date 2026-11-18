//! Serde helper functions

use serde::{Deserialize, Deserializer};

/// Deserializes an optional value, treating empty strings as `None`.
///
/// This is useful when external APIs return `"pubkey": ""` instead of `null`
/// or omitting the field entirely.
pub fn deserialize_empty_string_as_none<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    // First deserialize as an Option<String> to handle both null and string values
    let opt: Option<String> = Option::deserialize(deserializer)?;

    match opt {
        None => Ok(None),
        Some(s) if s.is_empty() => Ok(None),
        Some(s) => s.parse::<T>().map(Some).map_err(serde::de::Error::custom),
    }
}

/// Deserializes a flattened `extra` map, treating an empty map as `Value::Null`.
///
/// A flattened `serde_json::Value` always captures the leftover entries as an
/// object, so without this an absent `extra` would come back as `{}` and fail to
/// compare equal to the `Null` it was serialized from.
pub fn deserialize_empty_map_as_null<'de, D>(deserializer: D) -> Result<serde_json::Value, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;

    Ok(match value {
        serde_json::Value::Object(map) if map.is_empty() => serde_json::Value::Null,
        other => other,
    })
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;
    use crate::PublicKey;

    #[derive(Debug, Deserialize, PartialEq)]
    struct TestStruct {
        #[serde(default, deserialize_with = "deserialize_empty_string_as_none")]
        pubkey: Option<PublicKey>,
    }

    #[test]
    fn test_empty_string_as_none() {
        let json = r#"{"pubkey": ""}"#;
        let result: TestStruct = serde_json::from_str(json).unwrap();
        assert_eq!(result.pubkey, None);
    }

    #[test]
    fn test_null_as_none() {
        let json = r#"{"pubkey": null}"#;
        let result: TestStruct = serde_json::from_str(json).unwrap();
        assert_eq!(result.pubkey, None);
    }

    #[test]
    fn test_missing_field_as_none() {
        let json = r#"{}"#;
        let result: TestStruct = serde_json::from_str(json).unwrap();
        assert_eq!(result.pubkey, None);
    }

    #[test]
    fn test_valid_pubkey() {
        let json =
            r#"{"pubkey": "02194603ffa36356f4a56b7df9371fc3192472351453ec7398b8da8117e7c3e104"}"#;
        let result: TestStruct = serde_json::from_str(json).unwrap();
        assert!(result.pubkey.is_some());
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct Flattened {
        name: String,
        #[serde(flatten, default, deserialize_with = "deserialize_empty_map_as_null")]
        extra: serde_json::Value,
    }

    #[test]
    fn test_absent_extra_fields_become_null() {
        let parsed: Flattened = serde_json::from_str(r#"{"name": "a"}"#).unwrap();
        assert_eq!(parsed.extra, serde_json::Value::Null);
    }

    #[test]
    fn test_present_extra_fields_are_kept() {
        let parsed: Flattened = serde_json::from_str(r#"{"name": "a", "ref": 42}"#).unwrap();
        assert_eq!(parsed.extra, serde_json::json!({"ref": 42}));
    }
}

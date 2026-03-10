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
}

//! Secret

use std::fmt;
use std::str::FromStr;

use bitcoin::secp256k1::rand::{self, RngCore};
use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

use crate::util::hex;

/// The secret data that allows spending ecash
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct Secret(String);

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        if s.chars().count() > crate::nuts::nut00::MAX_SECRET_LENGTH {
            return Err(serde::de::Error::custom(
                "Secret exceeds maximum allowed length",
            ));
        }
        Ok(Self(s))
    }
}

/// Secret Errors
#[derive(Debug, Error)]
pub enum Error {
    /// Invalid Length
    #[error("Invalid secret length: `{0}`")]
    InvalidLength(u64),
    /// Invalid Secret
    #[error("Secret exceeds maximum allowed char length")]
    InvalidSecret,
    /// Hex Error
    #[error(transparent)]
    Hex(#[from] hex::Error),
    /// Serde Json error
    #[error(transparent)]
    SerdeJsonError(#[from] serde_json::Error),
}

impl Default for Secret {
    fn default() -> Self {
        Self::generate()
    }
}

impl Secret {
    /// Create new [`Secret`]
    #[inline]
    pub fn new<S>(secret: S) -> Result<Self, Error>
    where
        S: Into<String>,
    {
        let secret_str = secret.into();
        if secret_str.chars().count() > crate::nuts::nut00::MAX_SECRET_LENGTH {
            return Err(Error::InvalidSecret);
        }
        Ok(Self(secret_str))
    }

    /// Create secret value
    /// Generate a new random secret as the recommended 32 byte hex
    pub fn generate() -> Self {
        let mut rng = rand::thread_rng();

        let mut random_bytes = [0u8; 32];

        // Generate random bytes
        rng.fill_bytes(&mut random_bytes);
        // The secret string is hex encoded
        let secret = hex::encode(random_bytes);
        // This will always be valid as it's 64 bytes (32 bytes as hex)
        Self(secret)
    }

    /// [`Secret`] as bytes
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    /// [`Secret`] to bytes
    #[inline]
    pub fn to_bytes(&self) -> Vec<u8> {
        self.as_bytes().to_vec()
    }

    /// Check if secret is P2PK secret
    pub fn is_p2pk(&self) -> bool {
        use crate::nuts::Kind;

        let secret: Result<crate::nuts::nut10::Secret, serde_json::Error> =
            serde_json::from_str(&self.0);

        if let Ok(secret) = secret {
            if secret.kind.eq(&Kind::P2PK) {
                return true;
            }
        }

        false
    }
}

impl FromStr for Secret {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Secret> for Vec<u8> {
    fn from(value: Secret) -> Vec<u8> {
        value.to_bytes()
    }
}

impl From<&Secret> for Vec<u8> {
    fn from(value: &Secret) -> Vec<u8> {
        value.to_bytes()
    }
}

impl TryFrom<Secret> for crate::nuts::nut10::Secret {
    type Error = serde_json::Error;

    fn try_from(unchecked_secret: Secret) -> Result<crate::nuts::nut10::Secret, Self::Error> {
        serde_json::from_str(&unchecked_secret.0)
    }
}

impl TryFrom<&Secret> for crate::nuts::nut10::Secret {
    type Error = Error;

    fn try_from(unchecked_secret: &Secret) -> Result<crate::nuts::nut10::Secret, Self::Error> {
        Ok(serde_json::from_str(&unchecked_secret.0)?)
    }
}

#[cfg(test)]
mod tests {
    use std::assert_eq;
    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_secret_from_str() {
        let secret = Secret::generate();

        let secret_str = secret.to_string();

        assert_eq!(hex::decode(secret_str.clone()).unwrap().len(), 32);

        let secret_n = Secret::from_str(&secret_str).unwrap();

        assert_eq!(secret_n, secret)
    }

    #[test]
    fn test_secret_length_validation() {
        // Create a string that is exactly MAX_SECRET_LENGTH characters
        let max_length_string = "a".repeat(crate::nuts::nut00::MAX_SECRET_LENGTH);
        let secret_result = Secret::from_str(&max_length_string);
        assert!(
            secret_result.is_ok(),
            "Secret with max length should be valid"
        );

        // Create a string that is MAX_SECRET_LENGTH + 1 characters
        let too_long_string = "a".repeat(crate::nuts::nut00::MAX_SECRET_LENGTH + 1);
        let secret_result = Secret::from_str(&too_long_string);
        assert!(
            secret_result.is_err(),
            "Secret exceeding max length should be rejected"
        );

        match secret_result {
            Err(Error::InvalidSecret) => {} // Expected error
            Err(e) => panic!("Unexpected error type: {:?}", e),
            Ok(_) => panic!("Expected an error for too long secret"),
        }

        // Test with multi-byte characters (emoji)
        let emoji_string = "😀".repeat(crate::nuts::nut00::MAX_SECRET_LENGTH);
        let secret_result = Secret::from_str(&emoji_string);
        assert!(
            secret_result.is_ok(),
            "Secret with max length of emoji characters should be valid"
        );

        let too_long_emoji = "😀".repeat(crate::nuts::nut00::MAX_SECRET_LENGTH + 1);
        let secret_result = Secret::from_str(&too_long_emoji);
        assert!(
            secret_result.is_err(),
            "Secret exceeding max length with emoji should be rejected"
        );
    }

    #[test]
    fn test_secret_serde_deserialization_validation() {
        // Create a string that is exactly MAX_SECRET_LENGTH characters
        let max_length_string = "a".repeat(crate::nuts::nut00::MAX_SECRET_LENGTH);
        let json = format!("\"{}\"", max_length_string);
        let secret_result: Result<Secret, _> = serde_json::from_str(&json);
        assert!(
            secret_result.is_ok(),
            "Secret with max length should deserialize correctly"
        );

        // Create a string that is MAX_SECRET_LENGTH + 1 characters
        let too_long_string = "a".repeat(crate::nuts::nut00::MAX_SECRET_LENGTH + 1);
        let json = format!("\"{}\"", too_long_string);
        let secret_result: Result<Secret, _> = serde_json::from_str(&json);
        assert!(
            secret_result.is_err(),
            "Secret exceeding max length should fail deserialization"
        );

        // Test with multi-byte characters (emoji)
        let emoji_string = "😀".repeat(crate::nuts::nut00::MAX_SECRET_LENGTH);
        let json = format!("\"{}\"", emoji_string);
        let secret_result: Result<Secret, _> = serde_json::from_str(&json);
        assert!(
            secret_result.is_ok(),
            "Secret with max length of emoji characters should deserialize correctly"
        );

        let too_long_emoji = "😀".repeat(crate::nuts::nut00::MAX_SECRET_LENGTH + 1);
        let json = format!("\"{}\"", too_long_emoji);
        let secret_result: Result<Secret, _> = serde_json::from_str(&json);
        assert!(
            secret_result.is_err(),
            "Secret exceeding max length with emoji should fail deserialization"
        );
    }
}

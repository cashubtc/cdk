//! Secret

use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The secret data that allows spending ecash
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(pub String);

#[derive(Debug, Error)]
pub enum Error {
    #[error("Invalid secret length: `{0}`")]
    InvalidLength(u64),
    #[error("Hex error: `{0}`")]
    Hex(#[from] hex::FromHexError),
}

impl Default for Secret {
    fn default() -> Self {
        Self::new()
    }
}

impl Secret {
    /// Create secret value
    /// Generate a new random secret as the recommended 32 byte hex
    pub fn new() -> Self {
        use rand::RngCore;

        let mut rng = rand::thread_rng();

        let mut random_bytes = [0u8; 32];

        // Generate random bytes
        rng.fill_bytes(&mut random_bytes);
        // The secret string is hex encoded
        let secret = hex::encode(random_bytes);
        Self(secret)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.0.clone().into_bytes()
    }

    #[cfg(feature = "nut11")]
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
        Ok(Secret(s.to_string()))
    }
}

impl ToString for Secret {
    fn to_string(&self) -> String {
        self.0.clone()
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

#[cfg(feature = "nut10")]
impl TryFrom<Secret> for crate::nuts::nut10::Secret {
    type Error = serde_json::Error;

    fn try_from(unchecked_secret: Secret) -> Result<crate::nuts::nut10::Secret, Self::Error> {
        serde_json::from_str(&unchecked_secret.0)
    }
}

#[cfg(feature = "nut10")]
impl TryFrom<&Secret> for crate::nuts::nut10::Secret {
    type Error = serde_json::Error;

    fn try_from(unchecked_secret: &Secret) -> Result<crate::nuts::nut10::Secret, Self::Error> {
        serde_json::from_str(&unchecked_secret.0)
    }
}

#[cfg(test)]
mod tests {
    use std::assert_eq;
    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_secret_from_str() {
        let secret = Secret::new();

        let secret_str = secret.to_string();

        assert_eq!(hex::decode(secret_str.clone()).unwrap().len(), 32);

        let secret_n = Secret::from_str(&secret_str).unwrap();

        assert_eq!(secret_n, secret)
    }
}

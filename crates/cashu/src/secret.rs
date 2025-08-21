//! Secret

use std::fmt;
use std::str::FromStr;

use bitcoin::secp256k1::rand::{self, RngCore};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::Zeroize;

use crate::util::hex;

/// The secret data that allows spending ecash
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

/// Secret Errors
#[derive(Debug, Error)]
pub enum Error {
    /// Invalid Length
    #[error("Invalid secret length: `{0}`")]
    InvalidLength(u64),
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
    pub fn new<S>(secret: S) -> Self
    where
        S: Into<String>,
    {
        Self(secret.into())
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
            if secret.kind().eq(&Kind::P2PK) {
                return true;
            }
        }

        false
    }
}

impl FromStr for Secret {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.to_string()))
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

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
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
}

//! Secret

use std::str::FromStr;

use bip32::{DerivationPath, XPrv};
use bip39::Mnemonic;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::nuts::Id;

/// The secret data that allows spending ecash
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

#[derive(Debug, Error)]
pub enum Error {
    #[error("Invalid secret length: `{0}`")]
    InvalidLength(u64),
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

    pub fn from_seed(mnemonic: &Mnemonic, keyset_id: Id, counter: u64) -> Self {
        let path = DerivationPath::from_str(&format!(
            "m/129372'/0'/{}'/{}'/0",
            u64::from(keyset_id),
            counter
        ))
        .unwrap();

        let xpriv = XPrv::derive_from_path(mnemonic.to_seed(""), &path).unwrap();

        Self(hex::encode(xpriv.private_key().to_bytes()))
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
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

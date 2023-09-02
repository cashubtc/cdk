//! Mint public key exchange
// https://github.com/cashubtc/nuts/blob/main/01.md

use std::collections::BTreeMap;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::Amount;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PublicKey(#[serde(with = "crate::serde_utils::serde_public_key")] k256::PublicKey);

impl From<PublicKey> for k256::PublicKey {
    fn from(value: PublicKey) -> k256::PublicKey {
        value.0
    }
}

impl From<&PublicKey> for k256::PublicKey {
    fn from(value: &PublicKey) -> k256::PublicKey {
        value.0
    }
}

impl From<k256::PublicKey> for PublicKey {
    fn from(value: k256::PublicKey) -> Self {
        Self(value)
    }
}

impl PublicKey {
    // HACK: Fix the from_hex and to_hex
    // just leaving this hack for now as this pr is big enough
    pub fn from_hex(hex: String) -> Result<Self, Error> {
        let hex = hex::decode(hex)?;

        Ok(PublicKey(k256::PublicKey::from_sec1_bytes(&hex).unwrap()))
    }

    pub fn to_hex(&self) -> Result<String, Error> {
        Ok(serde_json::to_string(&self)?)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretKey(#[serde(with = "crate::serde_utils::serde_secret_key")] k256::SecretKey);

impl From<SecretKey> for k256::SecretKey {
    fn from(value: SecretKey) -> k256::SecretKey {
        value.0
    }
}

impl From<k256::SecretKey> for SecretKey {
    fn from(value: k256::SecretKey) -> Self {
        Self(value)
    }
}

// REVIEW: Guessing this is broken as well since its the same as pubkey
impl SecretKey {
    pub fn from_hex(hex: String) -> Result<Self, Error> {
        Ok(serde_json::from_str(&hex)?)
    }

    pub fn to_hex(&self) -> Result<String, Error> {
        Ok(serde_json::to_string(&self)?)
    }

    pub fn public_key(&self) -> PublicKey {
        self.0.public_key().into()
    }
}

/// Mint Keys [NUT-01]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Keys(BTreeMap<u64, PublicKey>);

impl Keys {
    pub fn new(keys: BTreeMap<u64, PublicKey>) -> Self {
        Self(keys)
    }

    pub fn keys(&self) -> BTreeMap<u64, PublicKey> {
        self.0.clone()
    }

    pub fn amount_key(&self, amount: Amount) -> Option<PublicKey> {
        self.0.get(&amount.to_sat()).cloned()
    }

    /// As serialized hashmap
    pub fn as_hashmap(&self) -> HashMap<u64, String> {
        self.0
            .iter()
            .map(|(k, v)| (k.to_owned(), hex::encode(v.0.to_sec1_bytes())))
            .collect()
    }
}

impl From<mint::Keys> for Keys {
    fn from(keys: mint::Keys) -> Self {
        Self(
            keys.0
                .iter()
                .map(|(amount, keypair)| (*amount, keypair.public_key.clone()))
                .collect(),
        )
    }
}

pub mod mint {
    use std::collections::BTreeMap;

    use serde::Serialize;

    use super::PublicKey;
    use super::SecretKey;
    use serde::Deserialize;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct Keys(pub BTreeMap<u64, KeyPair>);

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct KeyPair {
        pub public_key: PublicKey,
        pub secret_key: SecretKey,
    }

    impl KeyPair {
        pub fn from_secret_key(secret_key: SecretKey) -> Self {
            Self {
                public_key: secret_key.public_key(),
                secret_key,
            }
        }
    }
}

#[cfg(test)]
mod tests {

    use super::PublicKey;

    #[test]
    fn pubkey() {
        let pubkey = PublicKey::from_hex(
            "02c020067db727d586bc3183aecf97fcb800c3f4cc4759f69c626c9db5d8f5b5d4".to_string(),
        )
        .unwrap();
    }
}

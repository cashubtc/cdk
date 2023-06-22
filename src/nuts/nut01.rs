//! Mint public key exchange
// https://github.com/cashubtc/nuts/blob/main/01.md

use std::collections::BTreeMap;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

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

    pub fn amount_key(&self, amount: &u64) -> Option<PublicKey> {
        self.0.get(amount).cloned()
    }

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

    use k256::SecretKey;
    use serde::Deserialize;
    use serde::Serialize;

    use super::PublicKey;
    use crate::serde_utils;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct Keys(pub BTreeMap<u64, KeyPair>);

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct KeyPair {
        pub public_key: PublicKey,
        #[serde(with = "serde_utils::serde_secret_key")]
        pub secret_key: SecretKey,
    }

    impl KeyPair {
        pub fn from_secret_key(secret_key: SecretKey) -> Self {
            Self {
                public_key: secret_key.public_key().into(),
                secret_key,
            }
        }
    }
}

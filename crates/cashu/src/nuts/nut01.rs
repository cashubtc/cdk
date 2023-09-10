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
    pub fn from_hex(hex: String) -> Result<Self, Error> {
        let hex = hex::decode(hex)?;
        Ok(PublicKey(k256::PublicKey::from_sec1_bytes(&hex)?))
    }

    pub fn to_hex(&self) -> String {
        let bytes = self.0.to_sec1_bytes();
        hex::encode(bytes)
    }
}

impl std::fmt::Display for PublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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

impl SecretKey {
    pub fn to_hex(&self) -> String {
        let bytes = self.0.to_bytes();

        hex::encode(bytes)
    }

    pub fn public_key(&self) -> PublicKey {
        self.0.public_key().into()
    }
}

/// Mint Keys [NUT-01]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Keys(BTreeMap<Amount, PublicKey>);

impl Keys {
    pub fn new(keys: BTreeMap<Amount, PublicKey>) -> Self {
        Self(keys)
    }

    pub fn keys(&self) -> BTreeMap<Amount, PublicKey> {
        self.0.clone()
    }

    pub fn amount_key(&self, amount: Amount) -> Option<PublicKey> {
        self.0.get(&amount).cloned()
    }

    /// As serialized hashmap
    pub fn as_hashmap(&self) -> HashMap<Amount, String> {
        self.0
            .iter()
            .map(|(k, v)| (k.to_owned(), hex::encode(v.0.to_sec1_bytes())))
            .collect()
    }

    /// Iterate through the (`Amount`, `PublicKey`) entries in the Map
    pub fn iter(&self) -> impl Iterator<Item = (&Amount, &PublicKey)> {
        self.0.iter()
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

    use crate::Amount;

    use super::PublicKey;
    use super::SecretKey;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize)]
    pub struct Keys(pub BTreeMap<Amount, KeyPair>);

    #[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
        let pubkey_str = "02c020067db727d586bc3183aecf97fcb800c3f4cc4759f69c626c9db5d8f5b5d4";
        let pubkey = PublicKey::from_hex(pubkey_str.to_string()).unwrap();

        assert_eq!(pubkey_str, pubkey.to_hex())
    }
}

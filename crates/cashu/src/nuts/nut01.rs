//! Mint public key exchange
// https://github.com/cashubtc/nuts/blob/main/01.md

use std::collections::{BTreeMap, HashMap};

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

/// Mint Public Keys [NUT-01]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Response {
    /// set of public keys that the mint generates
    #[serde(flatten)]
    pub keys: Keys,
}

impl<'de> serde::de::Deserialize<'de> for Response {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct KeysVisitor;

        impl<'de> serde::de::Visitor<'de> for KeysVisitor {
            type Value = Response;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("")
            }

            fn visit_map<M>(self, mut m: M) -> Result<Self::Value, M::Error>
            where
                M: serde::de::MapAccess<'de>,
            {
                let mut keys: BTreeMap<Amount, PublicKey> = BTreeMap::new();

                while let Some((a, k)) = m.next_entry::<String, String>()? {
                    let amount = a.parse();
                    let pub_key = PublicKey::from_hex(k);

                    if let (Ok(amount), Ok(pubkey)) = (amount, pub_key) {
                        let amount = Amount::from_sat(amount);

                        keys.insert(amount, pubkey);
                    }
                    // TODO: Should return an error if an amount or key is
                    // invalid and not continue
                }

                Ok(Response { keys: Keys(keys) })
            }
        }

        deserializer.deserialize_map(KeysVisitor)
    }
}

pub mod mint {
    use std::collections::BTreeMap;

    use serde::Serialize;

    use super::{PublicKey, SecretKey};
    use crate::Amount;

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

    use super::*;

    #[test]
    fn pubkey() {
        let pubkey_str = "02c020067db727d586bc3183aecf97fcb800c3f4cc4759f69c626c9db5d8f5b5d4";
        let pubkey = PublicKey::from_hex(pubkey_str.to_string()).unwrap();

        assert_eq!(pubkey_str, pubkey.to_hex())
    }

    #[test]
    fn key_response() {
        let res: String = r#"{"1":"02f71e2d93aa95fc52b938735a24774ad926406c81e9dc9d2aa699fb89281548fd","2":"03b28dd9c19aaf1ec847be31b60c6a5e1a6cb6f87434afcdb0d9348ba0e2bdb150","4":"03ede0e704e223e764a82f73984b0fec0fdbde15ef57b4de95b527f7182af7487e","8":"020fd24fbd552445df70c244be2af77da2b2f634ccfda9e9620b347b5cd50dbdd8","16":"03ef9ef2515df5c0d0851ed9419a24a571ef5e03206d9d2fc6572ac050c5afe1aa","32":"02dbd455474176b30234c178573e874cc79d0c2fc1920cf0e9f133204cf43299c1","64":"0237c1eb11b8a214cca3e0104684227952188039a05cd55c1ad3896a572c70a7c3","128":"02655041771766b94a269f9f1ec1860f2eade55bb472c4db74ac1257ef54aac52b","256":"02b9e0be7b423bded7d60ff7114549de8d2d5b9c099edf8887aff474037e4bc0cf","512":"0320454cc41e646f49e1ac0a62b9667c80dee45545b045575f2a26f01770dc2521","1024":"0267fc1dabac016f46b3d1a650d97b56f3e56540106720f3d24ff7a6e9cd7183e9","2048":"035a9a25251a4da56f49667ca50677470fc6d8e186a875ab7b32aa064eb9e9e948","4096":"02f607a9eed310825c2d2e66d6e64fb237fe21b640b9a66cc7646b2a6480d91457","8192":"033346f7dce2ef71a80c5d657a8930bdd19c7c1708d03829daf43f20eaeda76768","16384":"024fad3b0b60c6b71d848deac173183fae8ddde31bbde531f18ab23473ddff541d","32768":"020d195466819d96d8c7eee9150565b7bd37196c7d12d0e96e389f56be8aebb44b","65536":"038c9bf295a745726c38d14988851d68d201296a802c296faa838000c2f44d25e0","131072":"032ff6491cdeff0bf9b34acd5deceef3cca7682b5f94dbe3068af8bb3b5aa34b81","262144":"02570090f5b6900955fd794d8f22c23fb35fc87fa03069b9b16bea63ea7cda419a","524288":"029d3c751c7d1c3e1d3e4b7791e1e809f6dedf2c28e172a82967d49a14b7c26ce2","1048576":"03b4a41d39cc6f2a8925f694c514e107b87d7ddb8f5ac55c9e4b7895139d0decd8","2097152":"02d4abbce491f87656eb0d2e66ef18eb009f6320169ef12e66703298d5395f2b91","4194304":"0359023fb85f6e6ede0141ab8f4a1277c19ed62b49b8ef5c5e2c8ca7fefe9b2f91","8388608":"0353d3ae1dad05e1b46ab85a366bfcdb7a645e3457f7714003e0fb06f4d75f4d87","16777216":"032d0847606465b97f15aca30c69f5baeeb43bf6188b4679f723119ce6fb9708c5","33554432":"028a673a53e78aa8c992128e21efb3b33fbd54de20afcf81a67e69eaf2bab7e0e9","67108864":"0278b66e140559352bb5aeca854a6466bc439ee206a9f349ed7926aae4335269b7","134217728":"023834651da0737f484a77204c2d06543fb65ad2dd8d095a2be48ca12ebf2664ec","268435456":"032cba9068638965ccc3870c140c72a1b028a820851f36fe59639e7ab3093a8ffd","536870912":"03eae5e4b22dfa5ad77476c925717dc4e005da78142e75b47fb28569d745483af3","1073741824":"02d17d61027602432a8484b65e6d6063ed9157c51ce92099d61ac2820411c59f9f","2147483648":"0236870e39b3a739d5caa04988dce432e3d7988420f04d9b415125af22672e2726"}"#.to_string();

        let response: Response = serde_json::from_str(&res).unwrap();

        assert_eq!(&serde_json::to_string(&response).unwrap(), &res)
    }
}

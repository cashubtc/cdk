//! Spend info (NUT-00, v3 keysets)
//!
//! A token entry for a v3 proof carries the spend info its receiver needs:
//!
//! | JSON   | V4  | Content                                   |
//! | ------ | --- | ----------------------------------------- |
//! | `k`    | `k` | bearer private key, 32 bytes              |
//! | `E`    | `e` | ephemeral public key, 33 bytes (NUT-28)   |
//! | `K`    | `i` | internal public key, 33 bytes             |
//! | `tree` | `t` | serialized leaves, in slot order          |
//! | `u`    | `u` | NUMS offset, 32 bytes                     |
//!
//! JSON carries hex strings, V4 tokens CBOR byte strings. Unknown fields are
//! ignored but preserved when re-serializing in the same format; they are
//! dropped when converting between JSON and CBOR, which encode values
//! differently.

use std::fmt;

use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use crate::nuts::nut01::{PublicKey, SecretKey};
use crate::nuts::nut10::nutroot::internal_key::nums_h;
use crate::nuts::nut10::nutroot::{self, merkle_root, tweak, tweaked_key, tweaked_secret_key};
use crate::nuts::nut10::nutroot::{Leaf, LeafBytes, ParsedLeaf, MAX_LEAVES};
use crate::nuts::nut28::slot_map::derive_slot_key;
use crate::util::hex;
use crate::SECP256K1;

/// Spend info error
#[derive(Debug, Error)]
pub enum Error {
    /// The spend info's shape is invalid
    #[error("Malformed spend info: {0}")]
    Malformed(&'static str),
    /// A field has the wrong length or is not a valid key
    #[error("Invalid spend info field `{0}`")]
    InvalidField(&'static str),
    /// The spend info does not rebuild the secret
    #[error("Spend info does not reconstruct the secret")]
    Mismatch,
    /// A disclosed `K` differs from the key derived from `E`
    #[error("Disclosed internal key differs from the derived one")]
    InternalKeyMismatch,
    /// `K - u*G` is not the NUMS point
    #[error("Internal key is not the NUMS offset of u")]
    NotNums,
    /// The proof is keyed to `E` and the wallet has no key to derive from it
    #[error("No receiver key to derive the internal key from E")]
    NoReceiverKey,
    /// The proof's secret is not a point
    #[error(transparent)]
    Secret(#[from] crate::secret::Error),
    /// A leaf or tree error
    #[error(transparent)]
    Nutroot(#[from] nutroot::Error),
    /// A slot derivation error
    #[error(transparent)]
    Nut28(#[from] crate::nuts::nut28::Error),
}

/// The spend info of a v3 proof
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SpendInfo {
    /// Bearer private key
    pub k: Option<SecretKey>,
    /// Ephemeral public key (receiver-keyed, NUT-28)
    pub e: Option<PublicKey>,
    /// Internal public key
    pub internal_key: Option<PublicKey>,
    /// Serialized leaves, in slot order
    pub tree: Option<Vec<LeafBytes>>,
    /// NUMS offset
    pub u: Option<SecretKey>,
    unknown: Unknown,
}

/// Unknown fields, kept in the format they arrived in
#[derive(Clone, Default)]
enum Unknown {
    #[default]
    None,
    Json(serde_json::Map<String, serde_json::Value>),
    Cbor(Vec<(ciborium::Value, ciborium::Value)>),
}

impl PartialEq for Unknown {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::None, Self::None) => true,
            (Self::Json(a), Self::Json(b)) => a == b,
            (Self::Cbor(a), Self::Cbor(b)) => a == b,
            _ => false,
        }
    }
}

// Unknown CBOR values are compared structurally; spend info never holds
// floats of its own, so equality is reflexive in practice.
impl Eq for Unknown {}

impl fmt::Debug for SpendInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpendInfo")
            .field("k", &self.k.as_ref().map(|_| "[REDACTED]"))
            .field("E", &self.e)
            .field("K", &self.internal_key)
            .field("tree", &self.tree)
            .field("u", &self.u.as_ref().map(|_| "[REDACTED]"))
            .finish_non_exhaustive()
    }
}

/// What check 1 reconstructed for a proof
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reconstruction {
    /// The internal key the secret is built from
    pub internal_key: PublicKey,
    /// How the internal key was conveyed
    pub source: KeySource,
    /// The secret key that spends by the key path, if the wallet holds it
    pub key_path_key: Option<SecretKey>,
    /// The disclosed leaves, parsed, in transmitted order
    pub leaves: Vec<Leaf>,
    /// Whether the internal key is a NUMS offset: no key path exists
    pub script_only: bool,
}

/// The route by which spend info conveys the internal key
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    /// `k*G` from a bearer `k`
    Bearer,
    /// Derived from `E` with the receiver's key (NUT-28 slot 0)
    ReceiverKeyed,
    /// `K` disclosed as is
    Disclosed,
}

impl SpendInfo {
    /// Bearer spend info: "here is the key"
    pub fn bearer(k: SecretKey) -> Self {
        Self {
            k: Some(k),
            ..Default::default()
        }
    }

    fn known_len(&self) -> usize {
        [
            self.k.is_some(),
            self.e.is_some(),
            self.internal_key.is_some(),
            self.u.is_some(),
            self.tree.is_some(),
        ]
        .into_iter()
        .filter(|present| *present)
        .count()
    }

    /// Whether unknown fields were received
    pub fn has_unknown_fields(&self) -> bool {
        !matches!(self.unknown, Unknown::None)
    }

    /// Check the shape rules
    ///
    /// `k` and `E` are exclusive; `tree` holds at least one and at most 8
    /// leaves; `u` needs a `tree`; a `tree` needs `k`, `E` or `K`.
    pub fn validate(&self) -> Result<(), Error> {
        if self.k.is_some() && self.e.is_some() {
            return Err(Error::Malformed("k and E are exclusive"));
        }
        match &self.tree {
            Some(tree) if tree.is_empty() => return Err(Error::Malformed("empty tree")),
            Some(tree) if tree.len() > MAX_LEAVES => {
                return Err(Error::Malformed("more than 8 leaves"))
            }
            Some(_) => {
                if self.k.is_none() && self.e.is_none() && self.internal_key.is_none() {
                    return Err(Error::Malformed("a tree needs k, E or K"));
                }
            }
            None => {
                if self.u.is_some() {
                    return Err(Error::Malformed("u without a tree"));
                }
            }
        }
        Ok(())
    }

    /// Receive check 1: rebuild `secret` from the spend info
    ///
    /// `receiver_key` is the wallet's static key, used when the proof is
    /// keyed to `E`. Rejects malformed spend info, leaves that do not parse
    /// as core leaves, and any mismatch with the secret. Points compare as
    /// full 33-byte encodings.
    pub fn reconstruct(
        &self,
        secret: &PublicKey,
        receiver_key: Option<&SecretKey>,
    ) -> Result<Reconstruction, Error> {
        self.validate()?;

        // The internal key, by precedence: bearer, receiver-keyed, disclosed.
        let (internal_key, source, internal_secret) = match (&self.k, &self.e) {
            (Some(k), _) => (k.public_key(), KeySource::Bearer, Some(k.clone())),
            (None, Some(e)) => {
                let receiver_key = receiver_key.ok_or(Error::NoReceiverKey)?;
                let derived = derive_slot_key(receiver_key, e, 0)?;
                let derived_key = derived.public_key();
                if self.internal_key.is_some_and(|k| k != derived_key) {
                    return Err(Error::InternalKeyMismatch);
                }
                (derived_key, KeySource::ReceiverKeyed, Some(derived))
            }
            (None, None) => match self.internal_key {
                Some(k) => (k, KeySource::Disclosed, None),
                None => return Err(Error::Malformed("no internal key source")),
            },
        };

        let Some(tree) = &self.tree else {
            // A bare key, or the aggregated form with the empty tweak.
            if internal_key == *secret {
                return Ok(Reconstruction {
                    internal_key,
                    source,
                    key_path_key: internal_secret,
                    leaves: vec![],
                    script_only: false,
                });
            }
            let t = tweak(&internal_key, None);
            if tweaked_key(&internal_key, &t)? != *secret {
                return Err(Error::Mismatch);
            }
            return Ok(Reconstruction {
                internal_key,
                source,
                key_path_key: internal_secret
                    .map(|k| tweaked_secret_key(&k, &t))
                    .transpose()?,
                leaves: vec![],
                script_only: false,
            });
        };

        let leaves = tree
            .iter()
            .map(|leaf| match leaf.parse()? {
                ParsedLeaf::Known(leaf) => Ok(leaf),
                ParsedLeaf::Unknown { version, leaf_type } => {
                    Err(nutroot::Error::UnknownLeaf { version, leaf_type }.into())
                }
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let root = merkle_root(tree)?;
        let t = tweak(&internal_key, Some(&root));
        if tweaked_key(&internal_key, &t)? != *secret {
            return Err(Error::Mismatch);
        }

        let script_only = match &self.u {
            Some(u) => {
                let neg_u_g = u.public_key().negate(&SECP256K1);
                if internal_key.combine(&neg_u_g).ok() != Some(nums_h()) {
                    return Err(Error::NotNums);
                }
                true
            }
            None => false,
        };

        Ok(Reconstruction {
            internal_key,
            source,
            key_path_key: match script_only {
                true => None,
                false => internal_secret
                    .map(|k| tweaked_secret_key(&k, &t))
                    .transpose()?,
            },
            leaves,
            script_only,
        })
    }
}

// --- JSON: hex strings ---

const JSON_K: &str = "k";
const JSON_E: &str = "E";
const JSON_INTERNAL_KEY: &str = "K";
const JSON_TREE: &str = "tree";
const JSON_U: &str = "u";

impl Serialize for SpendInfo {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let unknown = match &self.unknown {
            Unknown::Json(unknown) => unknown.len(),
            Unknown::Cbor(unknown) => {
                if !unknown.is_empty() {
                    tracing::warn!("Dropping unknown CBOR spend info fields when writing JSON");
                }
                0
            }
            Unknown::None => 0,
        };
        let mut map = serializer.serialize_map(Some(self.known_len() + unknown))?;
        if let Some(k) = &self.k {
            map.serialize_entry(JSON_K, &k.to_secret_hex())?;
        }
        if let Some(e) = &self.e {
            map.serialize_entry(JSON_E, &e.to_hex())?;
        }
        if let Some(internal_key) = &self.internal_key {
            map.serialize_entry(JSON_INTERNAL_KEY, &internal_key.to_hex())?;
        }
        if let Some(u) = &self.u {
            map.serialize_entry(JSON_U, &u.to_secret_hex())?;
        }
        if let Some(tree) = &self.tree {
            map.serialize_entry(JSON_TREE, tree)?;
        }
        if let Unknown::Json(unknown) = &self.unknown {
            for (key, value) in unknown {
                map.serialize_entry(key, value)?;
            }
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for SpendInfo {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::Error as _;

        let mut map = serde_json::Map::<String, serde_json::Value>::deserialize(deserializer)?;
        let mut take_hex = |key: &'static str| -> Result<Option<Vec<u8>>, D::Error> {
            match map.remove(key) {
                None => Ok(None),
                Some(serde_json::Value::String(s)) => hex::decode(s)
                    .map(Some)
                    .map_err(|_| D::Error::custom(Error::InvalidField(key))),
                Some(_) => Err(D::Error::custom(Error::InvalidField(key))),
            }
        };
        let k = take_hex(JSON_K)?;
        let e = take_hex(JSON_E)?;
        let internal_key = take_hex(JSON_INTERNAL_KEY)?;
        let u = take_hex(JSON_U)?;
        let tree = map
            .remove(JSON_TREE)
            .map(serde_json::from_value::<Vec<LeafBytes>>)
            .transpose()
            .map_err(D::Error::custom)?;
        let unknown = match map.is_empty() {
            true => Unknown::None,
            false => Unknown::Json(map),
        };
        Self::from_parts(k, e, internal_key, u, tree, unknown).map_err(D::Error::custom)
    }
}

impl SpendInfo {
    fn from_parts(
        k: Option<Vec<u8>>,
        e: Option<Vec<u8>>,
        internal_key: Option<Vec<u8>>,
        u: Option<Vec<u8>>,
        tree: Option<Vec<LeafBytes>>,
        unknown: Unknown,
    ) -> Result<Self, Error> {
        let scalar = |bytes: Option<Vec<u8>>, key| {
            bytes
                .map(|b| match b.len() {
                    32 => SecretKey::from_slice(&b).map_err(|_| Error::InvalidField(key)),
                    _ => Err(Error::InvalidField(key)),
                })
                .transpose()
        };
        let point = |bytes: Option<Vec<u8>>, key| {
            bytes
                .map(|b| match b.len() {
                    33 => PublicKey::from_slice(&b).map_err(|_| Error::InvalidField(key)),
                    _ => Err(Error::InvalidField(key)),
                })
                .transpose()
        };
        Ok(Self {
            k: scalar(k, "k")?,
            e: point(e, "E")?,
            internal_key: point(internal_key, "K")?,
            tree,
            u: scalar(u, "u")?,
            unknown,
        })
    }
}

// --- V4 tokens: CBOR byte strings ---

/// Spend info as the CBOR map `si` of a V4 token proof
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpendInfoV4(pub SpendInfo);

impl Serialize for SpendInfoV4 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let info = &self.0;
        let unknown = match &info.unknown {
            Unknown::Cbor(unknown) => unknown.len(),
            Unknown::Json(unknown) => {
                if !unknown.is_empty() {
                    tracing::warn!("Dropping unknown JSON spend info fields when writing CBOR");
                }
                0
            }
            Unknown::None => 0,
        };
        let mut map = serializer.serialize_map(Some(info.known_len() + unknown))?;
        if let Some(k) = &info.k {
            map.serialize_entry("k", serde_bytes::Bytes::new(&k.to_secret_bytes()))?;
        }
        if let Some(e) = &info.e {
            map.serialize_entry("e", serde_bytes::Bytes::new(&e.to_bytes()))?;
        }
        if let Some(internal_key) = &info.internal_key {
            map.serialize_entry("i", serde_bytes::Bytes::new(&internal_key.to_bytes()))?;
        }
        if let Some(u) = &info.u {
            map.serialize_entry("u", serde_bytes::Bytes::new(&u.to_secret_bytes()))?;
        }
        if let Some(tree) = &info.tree {
            let leaves: Vec<&serde_bytes::Bytes> = tree
                .iter()
                .map(|leaf| serde_bytes::Bytes::new(leaf.as_bytes()))
                .collect();
            map.serialize_entry("t", &leaves)?;
        }
        if let Unknown::Cbor(unknown) = &info.unknown {
            for (key, value) in unknown {
                map.serialize_entry(key, value)?;
            }
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for SpendInfoV4 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::Error as _;

        let ciborium::Value::Map(entries) = ciborium::Value::deserialize(deserializer)? else {
            return Err(D::Error::custom("spend info must be a map"));
        };
        let (mut k, mut e, mut internal_key, mut u, mut tree) = (None, None, None, None, None);
        let mut unknown = Vec::new();
        let bytes = |value: ciborium::Value, key: &'static str| match value {
            ciborium::Value::Bytes(bytes) => Ok(bytes),
            _ => Err(D::Error::custom(Error::InvalidField(key))),
        };
        for (key, value) in entries {
            match key.as_text() {
                Some("k") => k = Some(bytes(value, "k")?),
                Some("e") => e = Some(bytes(value, "E")?),
                Some("i") => internal_key = Some(bytes(value, "K")?),
                Some("u") => u = Some(bytes(value, "u")?),
                Some("t") => {
                    let ciborium::Value::Array(leaves) = value else {
                        return Err(D::Error::custom(Error::InvalidField("tree")));
                    };
                    tree = Some(
                        leaves
                            .into_iter()
                            .map(|leaf| {
                                LeafBytes::from_bytes(bytes(leaf, "tree")?)
                                    .map_err(D::Error::custom)
                            })
                            .collect::<Result<Vec<_>, _>>()?,
                    );
                }
                _ => unknown.push((key, value)),
            }
        }
        let unknown = match unknown.is_empty() {
            true => Unknown::None,
            false => Unknown::Cbor(unknown),
        };
        SpendInfo::from_parts(k, e, internal_key, u, tree, unknown)
            .map(Self)
            .map_err(D::Error::custom)
    }
}

/// Serde helpers for `Option<SpendInfo>` as the V4 CBOR map `si`
pub mod serde_v4 {
    use super::*;

    /// Serialize as the V4 map
    pub fn serialize<S>(info: &Option<SpendInfo>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match info {
            Some(info) => serializer.serialize_some(&SpendInfoV4(info.clone())),
            None => serializer.serialize_none(),
        }
    }

    /// Deserialize from the V4 map
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<SpendInfo>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Option::<SpendInfoV4>::deserialize(deserializer)?.map(|info| info.0))
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    const SECRET_K0: &str = "0240c186f3e19f6f8cd00db81c5266d5c74ddca48ff848efd0e3e125b2846f30d9";
    const K0: &str = "930c88ad9dc8531c147862d4ef9c0981200ff1f510c0a9e1c024db725a1d56c6";
    const E5: &str = "022f8bde4d1a07209355b4a7250a5c5128e88b84bddc619ab7cba8d569b240efe4";
    const AFTER_LEAF: &str = "00020200010104002102e493dbf1c10d80f3581e4904930b1404cc6c13900ee0758474fa94abe8c4cd1306000468a3be80";
    const WORKED_SECRET: &str =
        "02d310a4d661e3158e7d360617e739d6bacbf015431b24a43168db0ab99ef8f828";
    const WORKED_K: &str = "03a3e12cc077e5605f36441046f50c114fcc883b079a34028bed66732e3a419e51";

    fn key(n: u8) -> SecretKey {
        SecretKey::from_slice(&[[0u8; 31].as_slice(), &[n]].concat()).unwrap()
    }

    fn point(hex: &str) -> PublicKey {
        PublicKey::from_hex(hex).unwrap()
    }

    fn info(json: serde_json::Value) -> SpendInfo {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn json_round_trip_keeps_unknown_fields() {
        let json = serde_json::json!({"E": E5, "tree": [AFTER_LEAF], "x-future": {"a": 1}});
        let parsed = info(json.clone());
        assert!(parsed.has_unknown_fields());
        assert_eq!(serde_json::to_value(&parsed).unwrap(), json);
    }

    #[test]
    fn rejects_bad_field_encodings() {
        for json in [
            serde_json::json!({"k": "00"}),
            serde_json::json!({"k": 7}),
            serde_json::json!({"E": "02"}),
            serde_json::json!({"K": format!("04{}", &WORKED_K[2..])}),
            serde_json::json!({"u": "zz"}),
            serde_json::json!({"K": WORKED_K, "tree": ["00"]}),
        ] {
            assert!(
                serde_json::from_value::<SpendInfo>(json.clone()).is_err(),
                "{json}"
            );
        }
    }

    #[test]
    fn shape_rules() {
        let k = serde_json::json!(K0);
        for (json, ok) in [
            (serde_json::json!({"k": k, "E": E5}), false),
            (serde_json::json!({"K": WORKED_K, "tree": []}), false),
            (serde_json::json!({"K": WORKED_K, "u": K0}), false),
            (serde_json::json!({"tree": [AFTER_LEAF]}), false),
            (
                serde_json::json!({"K": WORKED_K, "tree": [AFTER_LEAF]}),
                true,
            ),
            (serde_json::json!({"k": k}), true),
            (serde_json::json!({}), true),
        ] {
            assert_eq!(info(json.clone()).validate().is_ok(), ok, "{json}");
        }
    }

    /// The bearer, receiver-keyed and disclosed shapes of tests/00
    #[test]
    fn check_1_vectors() {
        // Bearer: k*G is the secret.
        let r = info(serde_json::json!({"k": K0}))
            .reconstruct(&point(SECRET_K0), None)
            .unwrap();
        assert_eq!(r.source, KeySource::Bearer);
        assert_eq!(r.key_path_key.unwrap().public_key(), point(SECRET_K0));

        // Receiver-keyed, no conditions: the secret is slot 0 of key 3.
        let receiver = key(3);
        let r = info(serde_json::json!({"E": E5}))
            .reconstruct(&point(WORKED_K), Some(&receiver))
            .unwrap();
        assert_eq!(r.source, KeySource::ReceiverKeyed);
        assert_eq!(r.key_path_key.unwrap().public_key(), point(WORKED_K));
        // Another wallet cannot derive it.
        assert!(matches!(
            info(serde_json::json!({"E": E5})).reconstruct(&point(WORKED_K), Some(&key(4))),
            Err(Error::Mismatch)
        ));
        assert!(matches!(
            info(serde_json::json!({"E": E5})).reconstruct(&point(WORKED_K), None),
            Err(Error::NoReceiverKey)
        ));

        // Receiver-keyed with a tree: the worked example.
        let r = info(serde_json::json!({"E": E5, "tree": [AFTER_LEAF]}))
            .reconstruct(&point(WORKED_SECRET), Some(&receiver))
            .unwrap();
        assert_eq!(r.internal_key, point(WORKED_K));
        assert_eq!(
            r.key_path_key.unwrap().to_secret_hex(),
            "31b2e906239bae65b2d23718a037877b46a91b0b92f99fe8a899725f78d008e7"
        );
        assert_eq!(r.leaves.len(), 1);

        // Disclosed K with the tree: no key path for the holder.
        let r = info(serde_json::json!({"K": WORKED_K, "tree": [AFTER_LEAF]}))
            .reconstruct(&point(WORKED_SECRET), None)
            .unwrap();
        assert_eq!(r.source, KeySource::Disclosed);
        assert!(r.key_path_key.is_none());

        // A disclosed K that differs from the one derived from E rejects.
        assert!(matches!(
            info(serde_json::json!({"E": E5, "K": SECRET_K0, "tree": [AFTER_LEAF]}))
                .reconstruct(&point(WORKED_SECRET), Some(&receiver)),
            Err(Error::InternalKeyMismatch)
        ));
    }

    /// tests/00 script-only shape: K = H + u*G
    #[test]
    fn check_1_script_only_vector() {
        let r = info(serde_json::json!({
            "K": "031ed5e2846820073c39cca136f69aa3509b88b8f7f0e9f30159dea1178fa725f5",
            "u": "c4b697b346799e55a80ba7a0b6f7670a7f62a985c97de685dc693e87dc6d137a",
            "tree": [AFTER_LEAF],
        }))
        .reconstruct(
            &point("035f02db4e3b429de04669e8fba1e786719de8336cedeff3048e944ebcfaf08e23"),
            None,
        )
        .unwrap();
        assert!(r.script_only);
        assert!(r.key_path_key.is_none());

        // A u that does not open K to H rejects.
        assert!(matches!(
            info(serde_json::json!({
                "K": "031ed5e2846820073c39cca136f69aa3509b88b8f7f0e9f30159dea1178fa725f5",
                "u": K0,
                "tree": [AFTER_LEAF],
            }))
            .reconstruct(
                &point("035f02db4e3b429de04669e8fba1e786719de8336cedeff3048e944ebcfaf08e23"),
                None,
            ),
            Err(Error::NotNums)
        ));
    }

    #[test]
    fn check_1_rejections() {
        // Empty tweak: the aggregated form.
        let k3 = key(3);
        let aggregated =
            point("03b2bb251c006ae42d9c19f3157d02b3c347b1fb6512885225a8699a06d9233aee");
        let r = SpendInfo::bearer(k3.clone())
            .reconstruct(&aggregated, None)
            .unwrap();
        assert_eq!(r.key_path_key.unwrap().public_key(), aggregated);

        // Parity flipped: the 33-byte encodings differ.
        let mut flipped = aggregated.to_bytes();
        flipped[0] = 0x02;
        assert!(matches!(
            SpendInfo::bearer(k3.clone())
                .reconstruct(&PublicKey::from_slice(&flipped).unwrap(), None),
            Err(Error::Mismatch)
        ));

        // An unknown leaf type rejects on receive.
        let unknown =
            "00ff0200010104002102f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9";
        assert!(matches!(
            info(serde_json::json!({"K": WORKED_K, "tree": [unknown]}))
                .reconstruct(&point(WORKED_SECRET), None),
            Err(Error::Nutroot(nutroot::Error::UnknownLeaf { .. }))
        ));

        // A permuted tree is equivalent.
        let threshold =
            "00010200010104002102f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9";
        let leaves = [
            LeafBytes::from_str(AFTER_LEAF).unwrap(),
            LeafBytes::from_str(threshold).unwrap(),
        ];
        let root = merkle_root(&leaves).unwrap();
        let k = point(WORKED_K);
        let secret = tweaked_key(&k, &tweak(&k, Some(&root))).unwrap();
        for tree in [[AFTER_LEAF, threshold], [threshold, AFTER_LEAF]] {
            info(serde_json::json!({"K": WORKED_K, "tree": tree}))
                .reconstruct(&secret, None)
                .unwrap();
        }
    }
}

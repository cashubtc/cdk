//! Nutroot witnesses
//!
//! A witness is a serialized JSON object with no canonical encoding:
//!
//! ```json
//! {
//!   "signatures": ["<128-char hex BIP-340 signature>"],
//!   "leaf": "<serialized leaf, hex>",
//!   "control": { "K": "<33-byte hex>", "path": ["<32-byte hex>"] },
//!   "preimage": "<hex>"
//! }
//! ```
//!
//! A witness without `leaf` is a key-path witness.
//!
//! NUT-00 gives witnesses no rule for unknown fields. CDK rejects them, as
//! for leaves: a verifier that ignored a field a newer NUT defines could
//! accept a spend that a newer verifier evaluates differently.

use std::fmt;
use std::str::FromStr;

use bitcoin::secp256k1::schnorr::Signature;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::leaf::LeafBytes;
use super::tree::NodeHash;
use super::Error;
use crate::nuts::nut01::PublicKey;
use crate::util::hex;

/// A nutroot witness
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NutrootWitness {
    /// BIP-340 signatures over the input digest
    #[serde(with = "signatures_hex")]
    pub signatures: Vec<Signature>,
    /// The revealed leaf (script path only)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leaf: Option<LeafBytes>,
    /// The internal key and merkle path (script path only)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control: Option<Control>,
    /// The hashlock preimage (hashlock leaves only)
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "optional_bytes_hex"
    )]
    pub preimage: Option<Vec<u8>>,
}

/// The control block of a script-path witness
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Control {
    /// The internal key
    #[serde(rename = "K")]
    pub internal_key: PublicKey,
    /// Sibling hashes from the leaf to the root
    #[serde(with = "path_hex")]
    pub path: Vec<NodeHash>,
}

impl NutrootWitness {
    /// A key-path witness carrying one signature
    pub fn key_path(signature: Signature) -> Self {
        Self {
            signatures: vec![signature],
            leaf: None,
            control: None,
            preimage: None,
        }
    }

    /// Whether this is a key-path witness (no `leaf`)
    pub fn is_key_path(&self) -> bool {
        self.leaf.is_none()
    }

    /// Parse a mint quote's `signature` field
    ///
    /// It holds a serialized witness, or a key-path witness as the bare
    /// 128-character hex signature (NUT-00).
    pub fn from_quote_signature(signature: &str) -> Result<Self, Error> {
        if signature.len() == 128 && signature.bytes().all(|b| b.is_ascii_hexdigit()) {
            let bytes = hex::decode(signature)
                .map_err(|_| Error::MalformedWitness("invalid signature hex"))?;
            return Ok(Self::key_path(Signature::from_slice(&bytes)?));
        }
        Self::from_str(signature)
    }
}

/// A nutroot witness as received: its serialized JSON, checked to parse
///
/// Witnesses have no canonical encoding, so the mint keeps and echoes the
/// exact string it was sent (NUT-07).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RawNutrootWitness(String);

impl RawNutrootWitness {
    /// Wrap a serialized witness, rejecting one that does not parse
    pub fn new(raw: String) -> Result<Self, Error> {
        NutrootWitness::from_str(&raw)?;
        Ok(Self(raw))
    }

    /// The serialized witness
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Parse the witness
    pub fn parse(&self) -> Result<NutrootWitness, Error> {
        NutrootWitness::from_str(&self.0)
    }
}

impl From<&NutrootWitness> for RawNutrootWitness {
    fn from(witness: &NutrootWitness) -> Self {
        Self(witness.to_string())
    }
}

impl From<NutrootWitness> for RawNutrootWitness {
    fn from(witness: NutrootWitness) -> Self {
        Self::from(&witness)
    }
}

impl fmt::Debug for RawNutrootWitness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RawNutrootWitness({})", self.0)
    }
}

impl fmt::Display for RawNutrootWitness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for RawNutrootWitness {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RawNutrootWitness {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl FromStr for NutrootWitness {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(serde_json::from_str(s)?)
    }
}

impl fmt::Display for NutrootWitness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let json = serde_json::to_string(self).map_err(|_| fmt::Error)?;
        f.write_str(&json)
    }
}

mod signatures_hex {
    use super::*;

    pub(super) fn serialize<S>(signatures: &[Signature], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_seq(signatures.iter().map(|s| hex::encode(s.serialize())))
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Vec<Signature>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Vec::<String>::deserialize(deserializer)?
            .iter()
            .map(|s| {
                if s.len() != 128 {
                    return Err(serde::de::Error::custom(
                        "signature must be 128 hex characters",
                    ));
                }
                let bytes = hex::decode(s).map_err(serde::de::Error::custom)?;
                Signature::from_slice(&bytes).map_err(serde::de::Error::custom)
            })
            .collect()
    }
}

mod path_hex {
    use super::*;

    pub(super) fn serialize<S>(path: &[NodeHash], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_seq(path.iter().map(hex::encode))
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Vec<NodeHash>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Vec::<String>::deserialize(deserializer)?
            .iter()
            .map(|s| {
                let bytes = hex::decode(s).map_err(serde::de::Error::custom)?;
                NodeHash::try_from(bytes.as_slice())
                    .map_err(|_| serde::de::Error::custom("path hash must be 32 bytes"))
            })
            .collect()
    }
}

mod optional_bytes_hex {
    use super::*;

    pub(super) fn serialize<S>(bytes: &Option<Vec<u8>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match bytes {
            Some(bytes) => serializer.serialize_str(&hex::encode(bytes)),
            None => serializer.serialize_none(),
        }
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Option<Vec<u8>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<String>::deserialize(deserializer)?
            .map(|s| hex::decode(s).map_err(serde::de::Error::custom))
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIG: &str = "619e0726595b5adff06cc3e6ea1c409f10f7b064cf8888f0eed0efbac854eabf4632642930bdc7c4d7d983379301a4f263991dbd19d96e5ebcfab9e8583bd510";

    #[test]
    fn key_path_round_trip() {
        let json = format!(r#"{{"signatures":["{SIG}"]}}"#);
        let witness = NutrootWitness::from_str(&json).unwrap();
        assert!(witness.is_key_path());
        assert_eq!(witness.to_string(), json);
    }

    #[test]
    fn script_path_round_trip() {
        let json = r#"{"signatures":["0b2ea247bfca1264db86907aef4cb19935ed9a5b2043a757259dcdb5c599372c230602f2cfc0cd8b11aa98ff17fbed91f38f07db817263fcc7c50c846715873e"],"leaf":"00020200010104002102e493dbf1c10d80f3581e4904930b1404cc6c13900ee0758474fa94abe8c4cd1306000468a3be80","control":{"K":"03a3e12cc077e5605f36441046f50c114fcc883b079a34028bed66732e3a419e51","path":[]}}"#;
        let witness = NutrootWitness::from_str(json).unwrap();
        assert!(!witness.is_key_path());
        assert_eq!(witness.to_string(), json);
    }

    #[test]
    fn quote_signature_forms() {
        let bare = NutrootWitness::from_quote_signature(SIG).unwrap();
        let object =
            NutrootWitness::from_quote_signature(&format!(r#"{{"signatures":["{SIG}"]}}"#))
                .unwrap();
        assert_eq!(bare, object);
        assert!(NutrootWitness::from_quote_signature(&SIG[..126]).is_err());
    }

    #[test]
    fn malformed_witnesses() {
        for json in [
            // signature not 64 bytes
            format!(r#"{{"signatures":["{}"]}}"#, &SIG[..126]),
            format!(r#"{{"signatures":["{SIG}00"]}}"#),
            // missing signatures
            r#"{"leaf":"0001"}"#.to_string(),
            // path hash not 32 bytes
            format!(
                r#"{{"signatures":["{SIG}"],"leaf":"0001","control":{{"K":"03a3e12cc077e5605f36441046f50c114fcc883b079a34028bed66732e3a419e51","path":["00"]}}}}"#
            ),
            // control without K
            format!(r#"{{"signatures":["{SIG}"],"leaf":"0001","control":{{"path":[]}}}}"#),
            // preimage not hex
            format!(r#"{{"signatures":["{SIG}"],"preimage":"zz"}}"#),
        ] {
            assert!(NutrootWitness::from_str(&json).is_err(), "accepted {json}");
        }
    }

    #[test]
    fn unknown_fields_are_rejected() {
        for json in [
            format!(r#"{{"signatures":["{SIG}"],"template":"00"}}"#),
            format!(
                r#"{{"signatures":["{SIG}"],"leaf":"0001","control":{{"K":"03a3e12cc077e5605f36441046f50c114fcc883b079a34028bed66732e3a419e51","path":[],"extra":1}}}}"#
            ),
        ] {
            assert!(NutrootWitness::from_str(&json).is_err(), "accepted {json}");
            assert!(NutrootWitness::from_quote_signature(&json).is_err());
        }
    }
}

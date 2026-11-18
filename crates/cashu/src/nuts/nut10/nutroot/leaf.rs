//! Condition leaves
//!
//! ```text
//! leaf = leaf_version (1 byte) || leaf_type (1 byte) || field TLVs
//! ```
//!
//! The field TLVs are a NUT-00 field stream, and the exact byte string is
//! the leaf everywhere: in spend info, in a witness, and as the hash
//! preimage. [`LeafBytes`] keeps those bytes; [`Leaf`] is the parsed form of
//! a version `0x00` core leaf.

use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::tree::leaf_hash;
use super::Error;
use crate::nuts::nut00::tlv::{decode_uint, encode_uint, read_field_stream, write_record};
use crate::nuts::nut01::PublicKey;
use crate::util::hex;

/// The only leaf version defined
pub const LEAF_VERSION_0: u8 = 0x00;
/// Largest leaf body (the bytes after `leaf_version`)
pub const MAX_LEAF_BODY_LEN: usize = 512;
/// Largest `time` a leaf can carry, `2^53 - 1`
pub const MAX_LEAF_TIME: u64 = (1 << 53) - 1;

const FIELD_N: u8 = 0x02;
const FIELD_KEYS: u8 = 0x04;
const FIELD_TIME: u8 = 0x06;
const FIELD_HASH: u8 = 0x08;

/// Core leaf types of version `0x00`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum LeafType {
    /// Signatures by `n` distinct listed keys
    Threshold = 0x01,
    /// Locktime reached, plus `n` signatures
    After = 0x02,
    /// Preimage of `hash`, plus `n` signatures
    Hashlock = 0x03,
    /// Binds the tree to 32 bytes of outside context; never spendable
    Commit = 0x04,
}

impl LeafType {
    /// Every core leaf type
    pub const CORE: [Self; 4] = [Self::Threshold, Self::After, Self::Hashlock, Self::Commit];

    fn from_byte(byte: u8) -> Option<Self> {
        Self::CORE.into_iter().find(|t| *t as u8 == byte)
    }

    /// `leaf_version || leaf_type` as lowercase hex, as NUT-06 lists it
    pub fn signal(&self) -> String {
        hex::encode([LEAF_VERSION_0, *self as u8])
    }
}

/// A parsed version `0x00` core leaf
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Leaf {
    /// `threshold`: `n` of `keys`
    Threshold {
        /// Signature threshold
        n: u8,
        /// Listed keys
        keys: Vec<PublicKey>,
    },
    /// `after`: clock at or past `time`, plus `n` of `keys`
    After {
        /// Signature threshold
        n: u8,
        /// Listed keys
        keys: Vec<PublicKey>,
        /// Unix seconds
        time: u64,
    },
    /// `hashlock`: preimage of `hash`, plus `n` of `keys`
    Hashlock {
        /// Signature threshold
        n: u8,
        /// Listed keys
        keys: Vec<PublicKey>,
        /// SHA-256 digest
        hash: [u8; 32],
    },
    /// `commit`: 32 bytes of outside context
    Commit {
        /// Committed digest
        hash: [u8; 32],
    },
}

/// The outcome of parsing well-formed leaf bytes
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedLeaf {
    /// A version `0x00` core leaf
    Known(Leaf),
    /// A leaf of an unknown version or type. A tree containing it still
    /// commits; a mint treats it as unsatisfiable, a wallet receiving it
    /// rejects it.
    Unknown {
        /// Leaf version
        version: u8,
        /// Leaf type
        leaf_type: u8,
    },
}

impl Leaf {
    /// The leaf type
    pub fn leaf_type(&self) -> LeafType {
        match self {
            Self::Threshold { .. } => LeafType::Threshold,
            Self::After { .. } => LeafType::After,
            Self::Hashlock { .. } => LeafType::Hashlock,
            Self::Commit { .. } => LeafType::Commit,
        }
    }

    /// Listed keys (none for `commit`)
    pub fn keys(&self) -> &[PublicKey] {
        match self {
            Self::Threshold { keys, .. }
            | Self::After { keys, .. }
            | Self::Hashlock { keys, .. } => keys,
            Self::Commit { .. } => &[],
        }
    }

    /// Signature threshold (none for `commit`)
    pub fn threshold(&self) -> Option<u8> {
        match self {
            Self::Threshold { n, .. } | Self::After { n, .. } | Self::Hashlock { n, .. } => {
                Some(*n)
            }
            Self::Commit { .. } => None,
        }
    }

    /// Validate and serialize the leaf
    pub fn to_leaf_bytes(&self) -> Result<LeafBytes, Error> {
        self.validate()?;
        let mut fields = Vec::new();
        if let Some(n) = self.threshold() {
            write_record(&mut fields, FIELD_N, &[n])?;
            let keys: Vec<u8> = self.keys().iter().flat_map(PublicKey::to_bytes).collect();
            write_record(&mut fields, FIELD_KEYS, &keys)?;
        }
        match self {
            Self::After { time, .. } => write_record(&mut fields, FIELD_TIME, &encode_uint(*time))?,
            Self::Hashlock { hash, .. } | Self::Commit { hash } => {
                write_record(&mut fields, FIELD_HASH, hash)?
            }
            Self::Threshold { .. } => (),
        }
        let mut bytes = vec![LEAF_VERSION_0, self.leaf_type() as u8];
        bytes.extend_from_slice(&fields);
        let leaf = LeafBytes(bytes);
        leaf.check_len()?;
        Ok(leaf)
    }

    fn validate(&self) -> Result<(), Error> {
        if let Some(n) = self.threshold() {
            validate_keys(self.keys())?;
            if n == 0 || usize::from(n) > self.keys().len() {
                return Err(Error::MalformedLeaf("n must be in [1, number of keys]"));
            }
        }
        if let Self::After { time, .. } = self {
            if *time > MAX_LEAF_TIME {
                return Err(Error::MalformedLeaf("time exceeds 2^53 - 1"));
            }
        }
        Ok(())
    }
}

fn validate_keys(keys: &[PublicKey]) -> Result<(), Error> {
    if keys.is_empty() {
        return Err(Error::MalformedLeaf("keys must list at least one key"));
    }
    let mut xs = HashSet::new();
    for key in keys {
        if !xs.insert(key.x_only_public_key().serialize()) {
            return Err(Error::MalformedLeaf("two keys share an x-coordinate"));
        }
    }
    Ok(())
}

/// The exact bytes of a serialized leaf
///
/// Serializes as lowercase hex. Construction only checks the length bounds:
/// call [`LeafBytes::parse`] to validate the contents.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LeafBytes(Vec<u8>);

impl LeafBytes {
    /// Wrap leaf bytes, checking only that they hold a version and type and
    /// that the body is at most 512 bytes
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, Error> {
        let leaf = Self(bytes);
        leaf.check_len()?;
        Ok(leaf)
    }

    fn check_len(&self) -> Result<(), Error> {
        if self.0.len() < 2 {
            return Err(Error::MalformedLeaf("missing leaf version or type"));
        }
        let body = self.0.len() - 1;
        if body > MAX_LEAF_BODY_LEN {
            return Err(Error::LeafTooLong(body));
        }
        Ok(())
    }

    /// The serialized bytes
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// `tagged_hash("Cashu_NutrootLeaf", leaf)`
    pub fn hash(&self) -> [u8; 32] {
        leaf_hash(&self.0)
    }

    /// Parse the leaf, failing closed on malformed fields
    ///
    /// An unknown version or type is not an error here: it parses to
    /// [`ParsedLeaf::Unknown`] so that a tree containing it still commits.
    /// Callers that evaluate or accept a leaf reject it.
    pub fn parse(&self) -> Result<ParsedLeaf, Error> {
        self.check_len()?;
        let (version, leaf_type, fields) = (self.0[0], self.0[1], &self.0[2..]);
        let leaf_type = match (version, LeafType::from_byte(leaf_type)) {
            (LEAF_VERSION_0, Some(leaf_type)) => leaf_type,
            _ => return Ok(ParsedLeaf::Unknown { version, leaf_type }),
        };

        let mut n = None;
        let mut keys = None;
        let mut time = None;
        let mut hash = None;
        for record in read_field_stream(fields)? {
            match record.record_type {
                FIELD_N => {
                    let [value] = record.value else {
                        return Err(Error::MalformedLeaf("n must be 1 byte"));
                    };
                    n = Some(*value);
                }
                FIELD_KEYS => {
                    if record.value.is_empty() || record.value.len() % 33 != 0 {
                        return Err(Error::MalformedLeaf("keys must be 33-byte points"));
                    }
                    keys = Some(
                        record
                            .value
                            .chunks(33)
                            .map(|key| {
                                PublicKey::from_slice(key)
                                    .map_err(|_| Error::MalformedLeaf("invalid key"))
                            })
                            .collect::<Result<Vec<_>, _>>()?,
                    );
                }
                FIELD_TIME => time = Some(decode_uint(record.value)?),
                FIELD_HASH => {
                    hash = Some(
                        <[u8; 32]>::try_from(record.value)
                            .map_err(|_| Error::MalformedLeaf("hash must be 32 bytes"))?,
                    );
                }
                _ => return Err(Error::MalformedLeaf("unknown field")),
            }
        }

        let leaf = match (leaf_type, n, keys, time, hash) {
            (LeafType::Threshold, Some(n), Some(keys), None, None) => Leaf::Threshold { n, keys },
            (LeafType::After, Some(n), Some(keys), Some(time), None) => {
                Leaf::After { n, keys, time }
            }
            (LeafType::Hashlock, Some(n), Some(keys), None, Some(hash)) => {
                Leaf::Hashlock { n, keys, hash }
            }
            (LeafType::Commit, None, None, None, Some(hash)) => Leaf::Commit { hash },
            _ => return Err(Error::MalformedLeaf("fields do not match the leaf type")),
        };
        leaf.validate()?;
        Ok(ParsedLeaf::Known(leaf))
    }

    /// Parse a leaf that must be a known, spendable core leaf
    pub fn parse_known(&self) -> Result<Leaf, Error> {
        match self.parse()? {
            ParsedLeaf::Known(leaf) => Ok(leaf),
            ParsedLeaf::Unknown { version, leaf_type } => {
                Err(Error::UnknownLeaf { version, leaf_type })
            }
        }
    }
}

impl fmt::Debug for LeafBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LeafBytes({})", hex::encode(&self.0))
    }
}

impl fmt::Display for LeafBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(&self.0))
    }
}

impl FromStr for LeafBytes {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_bytes(hex::decode(s).map_err(|_| Error::MalformedLeaf("invalid hex"))?)
    }
}

impl TryFrom<Leaf> for LeafBytes {
    type Error = Error;

    fn try_from(leaf: Leaf) -> Result<Self, Self::Error> {
        leaf.to_leaf_bytes()
    }
}

impl Serialize for LeafBytes {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for LeafBytes {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::from_str(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY3: &str = "02f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9";
    const KEY4: &str = "02e493dbf1c10d80f3581e4904930b1404cc6c13900ee0758474fa94abe8c4cd13";

    fn key(hex: &str) -> PublicKey {
        PublicKey::from_hex(hex).unwrap()
    }

    fn round_trip(leaf: Leaf, expected: &str) {
        let bytes = leaf.to_leaf_bytes().unwrap();
        assert_eq!(bytes.to_string(), expected);
        assert_eq!(
            LeafBytes::from_str(expected).unwrap().parse().unwrap(),
            ParsedLeaf::Known(leaf)
        );
    }

    /// tests/10-tests.md "Serialized leaves"
    #[test]
    fn serialized_leaf_vectors() {
        let hash = [0xa1; 32];
        round_trip(
            Leaf::Threshold {
                n: 1,
                keys: vec![key(KEY3)],
            },
            "00010200010104002102f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9",
        );
        round_trip(
            Leaf::Threshold {
                n: 2,
                keys: vec![key(KEY3), key(KEY4)],
            },
            "00010200010204004202f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f902e493dbf1c10d80f3581e4904930b1404cc6c13900ee0758474fa94abe8c4cd13",
        );
        round_trip(
            Leaf::Hashlock {
                n: 1,
                keys: vec![key(KEY3)],
                hash,
            },
            "00030200010104002102f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9080020a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1",
        );
        round_trip(
            Leaf::After {
                n: 1,
                keys: vec![key(KEY4)],
                time: 1_755_561_600,
            },
            "00020200010104002102e493dbf1c10d80f3581e4904930b1404cc6c13900ee0758474fa94abe8c4cd1306000468a3be80",
        );
        round_trip(
            Leaf::Commit {
                hash: hex::decode(
                    "64451ff981aa92887b070762e97302df00a0ab97a853c323394b0a9cfaf46fae",
                )
                .unwrap()
                .try_into()
                .unwrap(),
            },
            "000408002064451ff981aa92887b070762e97302df00a0ab97a853c323394b0a9cfaf46fae",
        );
    }

    /// tests/10-tests.md "Rejection vectors"
    #[test]
    fn rejection_vectors() {
        let unknown_field = LeafBytes::from_str("00010200010104002102f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9090004deadbeef").unwrap();
        assert!(matches!(
            unknown_field.parse(),
            Err(Error::MalformedLeaf("unknown field"))
        ));

        let unknown_type = LeafBytes::from_str(
            "00ff0200010104002102f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9",
        )
        .unwrap();
        assert_eq!(
            unknown_type.parse().unwrap(),
            ParsedLeaf::Unknown {
                version: 0,
                leaf_type: 0xff
            }
        );
        assert!(matches!(
            unknown_type.parse_known(),
            Err(Error::UnknownLeaf {
                version: 0,
                leaf_type: 0xff
            })
        ));

        let commit_with_n = LeafBytes::from_str(
            "00040200010108002064451ff981aa92887b070762e97302df00a0ab97a853c323394b0a9cfaf46fae",
        )
        .unwrap();
        assert!(matches!(
            commit_with_n.parse(),
            Err(Error::MalformedLeaf(_))
        ));
    }

    #[test]
    fn malformed_leaves() {
        let k3 = &KEY3;
        let cases = [
            // n = 0
            format!("000102000100040021{k3}"),
            // n > keys
            format!("000102000102040021{k3}"),
            // n of two bytes
            format!("00010200020001040021{k3}"),
            // missing keys
            "0001020001 01".replace(' ', ""),
            // keys not a multiple of 33
            format!("000102000101040022{k3}00"),
            // invalid key (x = 5 is not on the curve)
            format!("000102000101040021020{}", "0".repeat(62) + "5"),
            // duplicate x-coordinate (02 and 03 of the same x)
            format!("000102000101040042{k3}03{}", &k3[2..]),
            // fields out of order
            format!("000104002102{}020001 01", &k3[2..]).replace(' ', ""),
            // after without time
            format!("000202000101040021{k3}"),
            // non-minimal time
            format!("000202000101040021{k3}0600020001"),
            // time above 2^53 - 1
            format!("000202000101040021{k3}06000720000000000000"),
            // threshold with hash
            format!("000102000101040021{k3}080020{}", "a1".repeat(32)),
            // hashlock hash of 31 bytes
            format!("000302000101040021{k3}08001f{}", "a1".repeat(31)),
            // truncated record
            format!("000102000101040021{}", &k3[..64]),
        ];
        for case in cases {
            let leaf = LeafBytes::from_str(&case).unwrap();
            assert!(leaf.parse().is_err(), "accepted {case}");
        }
        assert!(LeafBytes::from_str("00").is_err());
        assert!(LeafBytes::from_str("").is_err());
    }

    #[test]
    fn body_limit() {
        let mut bytes = vec![0x00, 0xfe];
        bytes.extend(vec![0u8; 511]);
        assert!(LeafBytes::from_bytes(bytes.clone()).is_ok());
        bytes.push(0);
        assert!(matches!(
            LeafBytes::from_bytes(bytes),
            Err(Error::LeafTooLong(513))
        ));
    }

    #[test]
    fn building_validates() {
        assert!(Leaf::Threshold {
            n: 2,
            keys: vec![key(KEY3)]
        }
        .to_leaf_bytes()
        .is_err());
        assert!(Leaf::Threshold { n: 1, keys: vec![] }
            .to_leaf_bytes()
            .is_err());
        assert!(Leaf::After {
            n: 1,
            keys: vec![key(KEY3)],
            time: MAX_LEAF_TIME + 1
        }
        .to_leaf_bytes()
        .is_err());
        // 16 keys: 2 + 3 + 3 + 16 * 33 = 536 bytes, over the body limit
        let keys: Vec<PublicKey> = (1u8..=16)
            .map(|i| {
                crate::nuts::nut01::SecretKey::from_slice(&[[0u8; 31].as_slice(), &[i]].concat())
                    .unwrap()
                    .public_key()
            })
            .collect();
        assert!(matches!(
            Leaf::Threshold { n: 1, keys }.to_leaf_bytes(),
            Err(Error::LeafTooLong(_))
        ));
    }

    #[test]
    fn signals() {
        let signals: Vec<String> = LeafType::CORE.iter().map(LeafType::signal).collect();
        assert_eq!(signals, vec!["0001", "0002", "0003", "0004"]);
    }
}

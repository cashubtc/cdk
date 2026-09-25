use std::collections::HashSet;

use bitcoin::secp256k1::PublicKey;

use super::{tagged_hash, Error};

/// Additional condition on a threshold of signatures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    /// Signatures alone satisfy the leaf.
    Threshold,
    /// Signatures are valid at or after this Unix timestamp.
    After(u64),
    /// Signatures and a preimage of this SHA-256 digest are required.
    Hashlock([u8; 32]),
    /// An inert commitment to external data; never spendable.
    Commit([u8; 32]),
}

/// Validated version-zero declarative condition leaf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leaf {
    threshold: u8,
    keys: Vec<PublicKey>,
    condition: Condition,
    disclosure: bool,
}

impl Leaf {
    /// Construct a canonical leaf. Commit leaves require zero keys and threshold.
    pub fn new(
        threshold: u8,
        keys: Vec<PublicKey>,
        condition: Condition,
        disclosure: bool,
    ) -> Result<Self, Error> {
        let leaf = Self {
            threshold,
            keys,
            condition,
            disclosure,
        };
        leaf.validate()?;
        Ok(leaf)
    }

    fn validate(&self) -> Result<(), Error> {
        if matches!(self.condition, Condition::Commit(_)) {
            if self.threshold != 0 || !self.keys.is_empty() || self.disclosure {
                return Err(Error::InvalidLeaf);
            }
        } else if self.threshold == 0 || usize::from(self.threshold) > self.keys.len() {
            return Err(Error::InvalidLeaf);
        }
        let mut keys = HashSet::new();
        if self
            .keys
            .iter()
            .any(|key| !keys.insert(key.x_only_public_key().0))
        {
            return Err(Error::InvalidLeaf);
        }
        if matches!(self.condition, Condition::After(time) if time > (1 << 53) - 1)
            || self.keys.len() > 15
            || self.to_bytes().len() > 513
        {
            return Err(Error::InvalidLeaf);
        }
        Ok(())
    }

    /// Parse a leaf, rejecting unknown fields and noncanonical encodings.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        if !(2..=513).contains(&bytes.len()) || bytes[0] != 0 {
            return Err(Error::InvalidLeaf);
        }
        let mut remaining = &bytes[2..];
        let mut last = 0;
        let (mut n, mut keys, mut time, mut hash, mut disclosure) = (None, None, None, None, false);
        while !remaining.is_empty() {
            if remaining.len() < 3 {
                return Err(Error::InvalidLeaf);
            }
            let tag = remaining[0];
            let len = usize::from(u16::from_be_bytes([remaining[1], remaining[2]]));
            if tag <= last || remaining.len() < 3 + len {
                return Err(Error::InvalidLeaf);
            }
            last = tag;
            let value = &remaining[3..3 + len];
            remaining = &remaining[3 + len..];
            match tag {
                2 if len == 1 => n = Some(value[0]),
                4 if len > 0 && len % 33 == 0 => {
                    keys = Some(
                        value
                            .chunks_exact(33)
                            .map(|key| PublicKey::from_slice(key).map_err(|_| Error::InvalidLeaf))
                            .collect::<Result<Vec<_>, _>>()?,
                    );
                }
                6 if len <= 7 && value.first() != Some(&0) => {
                    let mut number = [0; 8];
                    number[8 - len..].copy_from_slice(value);
                    time = Some(u64::from_be_bytes(number));
                }
                8 if len == 32 => hash = Some(value.try_into().map_err(|_| Error::InvalidLeaf)?),
                10 if value == [1] => disclosure = true,
                _ => return Err(Error::InvalidLeaf),
            }
        }
        let condition = match (bytes[1], time, hash) {
            (1, None, None) => Condition::Threshold,
            (2, Some(time), None) => Condition::After(time),
            (3, None, Some(hash)) => Condition::Hashlock(hash),
            (4, None, Some(hash)) if n.is_none() && keys.is_none() && !disclosure => {
                Condition::Commit(hash)
            }
            _ => return Err(Error::InvalidLeaf),
        };
        let leaf = match condition {
            Condition::Commit(_) => Self::new(0, vec![], condition, false)?,
            _ => Self::new(
                n.ok_or(Error::InvalidLeaf)?,
                keys.ok_or(Error::InvalidLeaf)?,
                condition,
                disclosure,
            )?,
        };
        Ok(leaf)
    }

    /// Canonical leaf serialization including its version and type.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = vec![
            0,
            match self.condition {
                Condition::Threshold => 1,
                Condition::After(_) => 2,
                Condition::Hashlock(_) => 3,
                Condition::Commit(_) => 4,
            },
        ];
        if !matches!(self.condition, Condition::Commit(_)) {
            record(&mut bytes, 2, &[self.threshold]);
            let keys: Vec<_> = self.keys.iter().flat_map(PublicKey::serialize).collect();
            record(&mut bytes, 4, &keys);
        }
        match self.condition {
            Condition::After(time) => record(&mut bytes, 6, &integer(time)),
            Condition::Hashlock(hash) | Condition::Commit(hash) => record(&mut bytes, 8, &hash),
            Condition::Threshold => {}
        }
        if self.disclosure {
            record(&mut bytes, 10, &[1]);
        }
        bytes
    }

    /// Tagged leaf hash.
    pub fn hash(&self) -> [u8; 32] {
        tagged_hash("Cashu_NutrootLeaf", &self.to_bytes())
    }
    /// Required number of distinct signers.
    pub fn threshold(&self) -> u8 {
        self.threshold
    }
    /// Listed compressed signing keys.
    pub fn keys(&self) -> &[PublicKey] {
        &self.keys
    }
    /// Additional condition or inert commitment.
    pub fn condition(&self) -> &Condition {
        &self.condition
    }
    /// Whether an exercised witness must be published.
    pub fn disclosure(&self) -> bool {
        self.disclosure
    }
}

pub(super) fn integer(value: u64) -> Vec<u8> {
    let bytes = value.to_be_bytes();
    bytes[bytes.iter().position(|b| *b != 0).unwrap_or(8)..].to_vec()
}

fn record(out: &mut Vec<u8>, tag: u8, value: &[u8]) {
    out.push(tag);
    out.extend_from_slice(&(value.len() as u16).to_be_bytes());
    out.extend_from_slice(value);
}

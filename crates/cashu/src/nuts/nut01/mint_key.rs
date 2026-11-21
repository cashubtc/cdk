use core::fmt;
use core::hash::{Hash, Hasher};
use core::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};

use super::{BlsG2PublicKey, Error, PublicKey};

/// A mint public key.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MintPublicKey {
    /// Secp256k1 mint key used by v1 and v2 keysets.
    Secp256k1(PublicKey),
    /// BLS12-381 G2 mint key used by v3 keysets.
    Bls(BlsG2PublicKey),
}

impl MintPublicKey {
    /// Parse a mint key from its compressed encoding.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, Error> {
        match bytes.len() {
            33 | 65 => Ok(Self::Secp256k1(PublicKey::from_slice(bytes)?)),
            96 => Ok(Self::Bls(BlsG2PublicKey::from_bytes(bytes)?)),
            found => Err(Error::InvalidPublicKeySize {
                expected: 33,
                found,
            }),
        }
    }

    /// Parse a mint key from hex.
    pub fn from_hex<S>(hex: S) -> Result<Self, Error>
    where
        S: AsRef<str>,
    {
        let bytes = crate::util::hex::decode(hex.as_ref())?;
        if bytes.len() == 65 {
            return Err(Error::InvalidPublicKeySize {
                expected: 33,
                found: 65,
            });
        }
        Self::from_slice(&bytes)
    }

    /// Return the compressed encoding.
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            Self::Secp256k1(key) => key.to_bytes().to_vec(),
            Self::Bls(key) => key.to_bytes().to_vec(),
        }
    }

    /// Return the key as hex.
    pub fn to_hex(&self) -> String {
        crate::util::hex::encode(self.to_bytes())
    }

    /// Return the secp256k1 key.
    pub fn as_secp256k1(&self) -> Result<PublicKey, Error> {
        match self {
            Self::Secp256k1(key) => Ok(*key),
            Self::Bls(_) => Err(Error::WrongKeyKind),
        }
    }

    /// Return the BLS12-381 G2 key.
    pub fn as_bls(&self) -> Result<BlsG2PublicKey, Error> {
        match self {
            Self::Bls(key) => Ok(*key),
            Self::Secp256k1(_) => Err(Error::WrongKeyKind),
        }
    }
}

impl From<PublicKey> for MintPublicKey {
    fn from(key: PublicKey) -> Self {
        Self::Secp256k1(key)
    }
}

impl From<bitcoin::secp256k1::PublicKey> for MintPublicKey {
    fn from(key: bitcoin::secp256k1::PublicKey) -> Self {
        Self::Secp256k1(key.into())
    }
}

impl Hash for MintPublicKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            Self::Secp256k1(key) => key.to_bytes().as_slice().hash(state),
            Self::Bls(key) => key.to_bytes().as_slice().hash(state),
        }
    }
}

impl PartialOrd for MintPublicKey {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MintPublicKey {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        match (self, other) {
            (Self::Secp256k1(left), Self::Secp256k1(right)) => {
                left.to_bytes().cmp(&right.to_bytes())
            }
            (Self::Bls(left), Self::Bls(right)) => left.to_bytes().cmp(&right.to_bytes()),
            (Self::Secp256k1(left), Self::Bls(right)) => {
                left.to_bytes().as_slice().cmp(right.to_bytes().as_slice())
            }
            (Self::Bls(left), Self::Secp256k1(right)) => {
                left.to_bytes().as_slice().cmp(right.to_bytes().as_slice())
            }
        }
    }
}

impl From<BlsG2PublicKey> for MintPublicKey {
    fn from(key: BlsG2PublicKey) -> Self {
        Self::Bls(key)
    }
}

impl fmt::Debug for MintPublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MintPublicKey({})", self.to_hex())
    }
}

impl fmt::Display for MintPublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for MintPublicKey {
    type Err = Error;

    fn from_str(hex: &str) -> Result<Self, Self::Err> {
        Self::from_hex(hex)
    }
}

impl Serialize for MintPublicKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for MintPublicKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let key = String::deserialize(deserializer)?;
        Self::from_hex(key).map_err(serde::de::Error::custom)
    }
}

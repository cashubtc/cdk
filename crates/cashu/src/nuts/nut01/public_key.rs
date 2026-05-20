use core::fmt;
use core::hash::{Hash, Hasher};
use core::str::FromStr;

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash as BitcoinHash;
use bitcoin::secp256k1::schnorr::Signature;
use bitcoin::secp256k1::{self, Message, Scalar, Secp256k1, XOnlyPublicKey};
use serde::{Deserialize, Deserializer, Serialize};

use super::{BlsG1PublicKey, BlsG2PublicKey, Error};
use crate::SECP256K1;

/// Protocol public key or point.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PublicKey {
    /// Secp256k1 compressed public key.
    Secp256k1(secp256k1::PublicKey),
    /// BLS12-381 G1 point, used for blinded messages and signatures.
    BlsG1(BlsG1PublicKey),
    /// BLS12-381 G2 point, used for mint public keys.
    BlsG2(BlsG2PublicKey),
}

impl Hash for PublicKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.to_bytes().hash(state);
    }
}

impl PartialOrd for PublicKey {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PublicKey {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.to_bytes().cmp(&other.to_bytes())
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({})", self.to_hex())
    }
}

impl From<secp256k1::PublicKey> for PublicKey {
    fn from(inner: secp256k1::PublicKey) -> Self {
        Self::Secp256k1(inner)
    }
}

impl From<BlsG1PublicKey> for PublicKey {
    fn from(inner: BlsG1PublicKey) -> Self {
        Self::BlsG1(inner)
    }
}

impl From<BlsG2PublicKey> for PublicKey {
    fn from(inner: BlsG2PublicKey) -> Self {
        Self::BlsG2(inner)
    }
}

impl PublicKey {
    /// Parse from compressed bytes.
    #[inline]
    pub fn from_slice(slice: &[u8]) -> Result<Self, Error> {
        match slice.len() {
            33 => Ok(Self::Secp256k1(secp256k1::PublicKey::from_slice(slice)?)),
            48 => Ok(Self::BlsG1(BlsG1PublicKey::from_bytes(slice)?)),
            96 => Ok(Self::BlsG2(BlsG2PublicKey::from_bytes(slice)?)),
            found => Err(Error::InvalidPublicKeySize {
                expected: 33,
                found,
            }),
        }
    }

    /// Parse from hex string.
    #[inline]
    pub fn from_hex<S>(hex: S) -> Result<Self, Error>
    where
        S: AsRef<str>,
    {
        let bytes = crate::util::hex::decode(hex.as_ref())?;
        Self::from_slice(&bytes)
    }

    /// Return compressed bytes.
    #[inline]
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            Self::Secp256k1(inner) => inner.serialize().to_vec(),
            Self::BlsG1(inner) => inner.to_bytes().to_vec(),
            Self::BlsG2(inner) => inner.to_bytes().to_vec(),
        }
    }

    /// Return uncompressed secp256k1 bytes.
    #[inline]
    pub fn to_uncompressed_bytes(&self) -> [u8; 65] {
        match self {
            Self::Secp256k1(inner) => inner.serialize_uncompressed(),
            Self::BlsG1(_) | Self::BlsG2(_) => panic!("BLS keys do not have secp bytes"),
        }
    }

    /// Return secp256k1 x-only public key.
    #[inline]
    pub fn x_only_public_key(&self) -> XOnlyPublicKey {
        match self {
            Self::Secp256k1(inner) => inner.x_only_public_key().0,
            Self::BlsG1(_) | Self::BlsG2(_) => panic!("BLS keys do not have x-only form"),
        }
    }

    /// Return secp256k1 x-only public key and parity.
    #[inline]
    pub fn x_only_public_key_with_parity(&self) -> (XOnlyPublicKey, secp256k1::Parity) {
        match self {
            Self::Secp256k1(inner) => inner.x_only_public_key(),
            Self::BlsG1(_) | Self::BlsG2(_) => panic!("BLS keys do not have x-only form"),
        }
    }

    /// Get public key as hex string.
    #[inline]
    pub fn to_hex(&self) -> String {
        crate::util::hex::encode(self.to_bytes())
    }

    /// Verify schnorr signature.
    pub fn verify(&self, msg: &[u8], sig: &Signature) -> Result<(), Error> {
        let Self::Secp256k1(inner) = self else {
            return Err(Error::WrongKeyKind);
        };
        let hash: Sha256Hash = BitcoinHash::hash(msg);
        let msg = Message::from_digest_slice(hash.as_ref())?;
        SECP256K1.verify_schnorr(sig, &msg, &inner.x_only_public_key().0)?;
        Ok(())
    }

    /// Add two secp256k1 public keys.
    pub fn combine(&self, other: &Self) -> Result<Self, secp256k1::Error> {
        match (self, other) {
            (Self::Secp256k1(a), Self::Secp256k1(b)) => Ok(a.combine(b)?.into()),
            _ => Err(secp256k1::Error::InvalidPublicKey),
        }
    }

    /// Tweak-multiply a secp256k1 public key.
    pub fn mul_tweak<C>(
        &self,
        secp: &Secp256k1<C>,
        tweak: &Scalar,
    ) -> Result<Self, secp256k1::Error>
    where
        C: secp256k1::Verification,
    {
        match self {
            Self::Secp256k1(inner) => Ok(inner.mul_tweak(secp, tweak)?.into()),
            Self::BlsG1(_) | Self::BlsG2(_) => Err(secp256k1::Error::InvalidPublicKey),
        }
    }

    /// Negate a secp256k1 public key.
    pub fn negate<C>(&self, secp: &Secp256k1<C>) -> Self
    where
        C: secp256k1::Verification,
    {
        match self {
            Self::Secp256k1(inner) => inner.negate(secp).into(),
            Self::BlsG1(_) | Self::BlsG2(_) => panic!("cannot negate BLS point with secp context"),
        }
    }

    /// Return the BLS G1 point.
    pub fn as_bls_g1(&self) -> Result<BlsG1PublicKey, Error> {
        match self {
            Self::BlsG1(point) => Ok(*point),
            Self::Secp256k1(_) | Self::BlsG2(_) => Err(Error::WrongKeyKind),
        }
    }

    /// Return the BLS G2 point.
    pub fn as_bls_g2(&self) -> Result<BlsG2PublicKey, Error> {
        match self {
            Self::BlsG2(point) => Ok(*point),
            Self::Secp256k1(_) | Self::BlsG1(_) => Err(Error::WrongKeyKind),
        }
    }
}

impl FromStr for PublicKey {
    type Err = Error;

    fn from_str(hex: &str) -> Result<Self, Self::Err> {
        Self::from_hex(hex)
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

impl Serialize for PublicKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for PublicKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let public_key: String = String::deserialize(deserializer)?;
        Self::from_hex(public_key).map_err(serde::de::Error::custom)
    }
}

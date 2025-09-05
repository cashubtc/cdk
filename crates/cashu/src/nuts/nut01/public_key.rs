use core::fmt;
use core::ops::Deref;
use core::str::FromStr;

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::schnorr::Signature;
use bitcoin::secp256k1::{self, Message, XOnlyPublicKey};
use serde::{Deserialize, Deserializer, Serialize};

use super::Error;
use crate::SECP256K1;

/// PublicKey
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct PublicKey {
    #[cfg_attr(feature = "swagger", schema(value_type = String))]
    inner: secp256k1::PublicKey,
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({})", self.to_hex())
    }
}

impl Deref for PublicKey {
    type Target = secp256k1::PublicKey;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<secp256k1::PublicKey> for PublicKey {
    fn from(inner: secp256k1::PublicKey) -> Self {
        Self { inner }
    }
}

impl PublicKey {
    /// Parse from `bytes`
    #[inline]
    pub fn from_slice(slice: &[u8]) -> Result<Self, Error> {
        Ok(Self {
            inner: secp256k1::PublicKey::from_slice(slice)?,
        })
    }

    /// Parse from `hex` string
    #[inline]
    pub fn from_hex<S>(hex: S) -> Result<Self, Error>
    where
        S: AsRef<str>,
    {
        let hex: &str = hex.as_ref();

        // Check size
        if hex.len() != 33 * 2 {
            return Err(Error::InvalidPublicKeySize {
                expected: 33,
                found: hex.len() / 2,
            });
        }

        Ok(Self {
            inner: secp256k1::PublicKey::from_str(hex)?,
        })
    }

    /// [`PublicKey`] to bytes
    #[inline]
    pub fn to_bytes(&self) -> [u8; 33] {
        self.inner.serialize()
    }

    /// To uncompressed bytes
    #[inline]
    pub fn to_uncompressed_bytes(&self) -> [u8; 65] {
        self.inner.serialize_uncompressed()
    }

    /// To [`XOnlyPublicKey`]
    #[inline]
    pub fn x_only_public_key(&self) -> XOnlyPublicKey {
        self.inner.x_only_public_key().0
    }

    /// Get public key as `hex` string
    #[inline]
    pub fn to_hex(&self) -> String {
        self.inner.to_string()
    }

    /// Verify schnorr signature
    pub fn verify(&self, msg: &[u8], sig: &Signature) -> Result<(), Error> {
        let hash: Sha256Hash = Sha256Hash::hash(msg);
        let msg = Message::from_digest_slice(hash.as_ref())?;
        SECP256K1.verify_schnorr(sig, &msg, &self.inner.x_only_public_key().0)?;
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    pub fn test_public_key_from_hex() {
        // Compressed
        assert!(PublicKey::from_hex(
            "02194603ffa36356f4a56b7df9371fc3192472351453ec7398b8da8117e7c3e104"
        )
        .is_ok());
    }

    #[test]
    pub fn test_invalid_public_key_from_hex() {
        // Uncompressed (is valid but is cashu must be compressed?)
        assert!(PublicKey::from_hex("04fd4ce5a16b65576145949e6f99f445f8249fee17c606b688b504a849cdc452de3625246cb2c27dac965cb7200a5986467eee92eb7d496bbf1453b074e223e481")
            .is_err())
    }
}

#[cfg(all(feature = "bench", test))]
mod benches {
    extern crate test;
    use test::{black_box, Bencher};

    use super::*;

    const HEX: &str = "02194603ffa36356f4a56b7df9371fc3192472351453ec7398b8da8117e7c3e104";

    #[bench]
    pub fn public_key_from_hex(bh: &mut Bencher) {
        bh.iter(|| {
            black_box(PublicKey::from_hex(HEX)).unwrap();
        });
    }
}

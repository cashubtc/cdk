use core::fmt;
use core::ops::Deref;
use core::str::FromStr;

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash as BitcoinHash;
use bitcoin::secp256k1::schnorr::Signature;
use bitcoin::secp256k1::{self, Message, Scalar, Secp256k1, XOnlyPublicKey};
use serde::{Deserialize, Deserializer, Serialize};

use super::Error;
use crate::SECP256K1;

/// A compressed secp256k1 public key.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PublicKey {
    inner: secp256k1::PublicKey,
}

impl Deref for PublicKey {
    type Target = secp256k1::PublicKey;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({})", self.to_hex())
    }
}

impl From<secp256k1::PublicKey> for PublicKey {
    fn from(inner: secp256k1::PublicKey) -> Self {
        Self { inner }
    }
}

impl PublicKey {
    /// Parse from bytes.
    ///
    /// Accepts compressed (33 bytes) or uncompressed (65 bytes) secp256k1 keys.
    #[inline]
    pub fn from_slice(slice: &[u8]) -> Result<Self, Error> {
        Ok(Self {
            inner: secp256k1::PublicKey::from_slice(slice)?,
        })
    }

    /// Parse from hex string.
    #[inline]
    pub fn from_hex<S>(hex: S) -> Result<Self, Error>
    where
        S: AsRef<str>,
    {
        let hex = hex.as_ref();
        if hex.len() != 66 {
            return Err(Error::InvalidPublicKeySize {
                expected: 33,
                found: hex.len() / 2,
            });
        }
        Ok(Self {
            inner: secp256k1::PublicKey::from_str(hex)?,
        })
    }

    /// Return compressed bytes.
    #[inline]
    pub fn to_bytes(&self) -> [u8; 33] {
        self.inner.serialize()
    }

    /// Return uncompressed secp256k1 bytes.
    #[inline]
    pub fn to_uncompressed_bytes(&self) -> [u8; 65] {
        self.inner.serialize_uncompressed()
    }

    /// Return secp256k1 x-only public key.
    #[inline]
    pub fn x_only_public_key(&self) -> XOnlyPublicKey {
        self.inner.x_only_public_key().0
    }

    /// Return secp256k1 x-only public key and parity.
    #[inline]
    pub fn x_only_public_key_with_parity(&self) -> (XOnlyPublicKey, secp256k1::Parity) {
        self.inner.x_only_public_key()
    }

    /// Get public key as hex string.
    #[inline]
    pub fn to_hex(&self) -> String {
        self.inner.to_string()
    }

    /// Verify schnorr signature.
    pub fn verify(&self, msg: &[u8], sig: &Signature) -> Result<(), Error> {
        let hash: Sha256Hash = BitcoinHash::hash(msg);
        let msg = Message::from_digest_slice(hash.as_ref())?;
        SECP256K1.verify_schnorr(sig, &msg, &self.inner.x_only_public_key().0)?;
        Ok(())
    }

    /// Add two secp256k1 public keys.
    pub fn combine(&self, other: &Self) -> Result<Self, secp256k1::Error> {
        Ok(self.inner.combine(&other.inner)?.into())
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
        Ok(self.inner.mul_tweak(secp, tweak)?.into())
    }

    /// Negate a secp256k1 public key.
    pub fn negate<C>(&self, secp: &Secp256k1<C>) -> Self
    where
        C: secp256k1::Verification,
    {
        self.inner.negate(secp).into()
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
    fn rejects_bls_point_lengths_before_curve_parsing() {
        assert!(matches!(
            PublicKey::from_hex("00".repeat(48)),
            Err(Error::InvalidPublicKeySize { found: 48, .. })
        ));
        assert!(matches!(
            PublicKey::from_hex("00".repeat(96)),
            Err(Error::InvalidPublicKeySize { found: 96, .. })
        ));
    }

    #[test]
    fn test_public_key_from_hex() {
        // Compressed
        let key = PublicKey::from_hex(
            "02194603ffa36356f4a56b7df9371fc3192472351453ec7398b8da8117e7c3e104",
        )
        .unwrap();

        assert_eq!(
            format!("{key:?}"),
            "PublicKey(02194603ffa36356f4a56b7df9371fc3192472351453ec7398b8da8117e7c3e104)"
        );
    }

    #[test]
    fn from_slice_accepts_uncompressed_secp256k1_keys() {
        let compressed = crate::nuts::SecretKey::generate().public_key();
        let uncompressed = compressed.serialize_uncompressed();

        let parsed = PublicKey::from_slice(&uncompressed).unwrap();
        assert_eq!(parsed, compressed);
        assert!(matches!(
            PublicKey::from_hex(crate::util::hex::encode(uncompressed)),
            Err(Error::InvalidPublicKeySize {
                expected: 33,
                found: 65
            })
        ));
    }

    #[test]
    fn test_invalid_public_key_from_hex() {
        // Uncompressed (is valid but is cashu must be compressed?)
        assert!(PublicKey::from_hex("04fd4ce5a16b65576145949e6f99f445f8249fee17c606b688b504a849cdc452de3625246cb2c27dac965cb7200a5986467eee92eb7d496bbf1453b074e223e481")
            .is_err());
        assert!(matches!(
            PublicKey::from_hex(
                "02194603ffa36356f4a56b7df9371fc3192472351453ec7398b8da8117e7c3e10400"
            ),
            Err(Error::InvalidPublicKeySize {
                expected: 33,
                found: 34
            })
        ));
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

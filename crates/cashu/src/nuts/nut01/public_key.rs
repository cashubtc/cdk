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
    ///
    /// Returns [`Error::WrongKeyKind`] for BLS public keys.
    #[inline]
    pub fn to_uncompressed_bytes(&self) -> Result<[u8; 65], Error> {
        match self {
            Self::Secp256k1(inner) => Ok(inner.serialize_uncompressed()),
            Self::BlsG1(_) | Self::BlsG2(_) => Err(Error::WrongKeyKind),
        }
    }

    /// Return secp256k1 x-only public key.
    ///
    /// Returns [`Error::WrongKeyKind`] for BLS public keys.
    #[inline]
    pub fn x_only_public_key(&self) -> Result<XOnlyPublicKey, Error> {
        match self {
            Self::Secp256k1(inner) => Ok(inner.x_only_public_key().0),
            Self::BlsG1(_) | Self::BlsG2(_) => Err(Error::WrongKeyKind),
        }
    }

    /// Return secp256k1 x-only public key and parity.
    ///
    /// Returns [`Error::WrongKeyKind`] for BLS public keys.
    #[inline]
    pub fn x_only_public_key_with_parity(
        &self,
    ) -> Result<(XOnlyPublicKey, secp256k1::Parity), Error> {
        match self {
            Self::Secp256k1(inner) => Ok(inner.x_only_public_key()),
            Self::BlsG1(_) | Self::BlsG2(_) => Err(Error::WrongKeyKind),
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
    ///
    /// Returns [`Error::WrongKeyKind`] for BLS public keys.
    pub fn negate<C>(&self, secp: &Secp256k1<C>) -> Result<Self, Error>
    where
        C: secp256k1::Verification,
    {
        match self {
            Self::Secp256k1(inner) => Ok(inner.negate(secp).into()),
            Self::BlsG1(_) | Self::BlsG2(_) => Err(Error::WrongKeyKind),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nuts::nut01::BlsSecretKey;

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

    #[test]
    fn secp_only_operations_reject_bls_public_keys() {
        let bls = PublicKey::from(BlsG1PublicKey::hash_to_curve(b"wrong secp key kind"));

        assert!(matches!(
            bls.to_uncompressed_bytes(),
            Err(Error::WrongKeyKind)
        ));
        assert!(matches!(bls.x_only_public_key(), Err(Error::WrongKeyKind)));
        assert!(matches!(
            bls.x_only_public_key_with_parity(),
            Err(Error::WrongKeyKind)
        ));
        assert!(matches!(bls.negate(&SECP256K1), Err(Error::WrongKeyKind)));
    }

    #[test]
    fn secp_and_bls_public_key_json_round_trip() {
        let secp = PublicKey::from_hex(
            "02194603ffa36356f4a56b7df9371fc3192472351453ec7398b8da8117e7c3e104",
        )
        .expect("secp key");
        let bls_g1 = PublicKey::from(BlsG1PublicKey::hash_to_curve(b"serde bls g1"));
        let bls_g2 = PublicKey::from(BlsSecretKey::from_reduced_bytes(&[7; 32]).public_key_g2());

        for key in [secp, bls_g1, bls_g2] {
            let encoded = serde_json::to_string(&key).expect("serialize public key");
            let decoded: PublicKey =
                serde_json::from_str(&encoded).expect("deserialize public key");
            assert_eq!(decoded, key);
        }
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

use core::fmt;
use core::str::FromStr;

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1;
use bitcoin::secp256k1::rand::rngs::OsRng;
use bitcoin::secp256k1::rand::RngCore;
use bitcoin::secp256k1::schnorr::Signature;
use bitcoin::secp256k1::{Keypair, Message, Scalar, Secp256k1, XOnlyPublicKey};
use serde::de::Visitor;
use serde::{Deserialize, Deserializer, Serialize};

use super::{BlsSecretKey, Error, PublicKey};
use crate::SECP256K1;

/// Protocol secret key/scalar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretKey {
    /// Secp256k1 secret key.
    Secp256k1(secp256k1::SecretKey),
    /// BLS12-381 scalar.
    Bls(BlsSecretKey),
}

impl From<secp256k1::SecretKey> for SecretKey {
    fn from(inner: secp256k1::SecretKey) -> Self {
        Self::Secp256k1(inner)
    }
}

impl From<BlsSecretKey> for SecretKey {
    fn from(inner: BlsSecretKey) -> Self {
        Self::Bls(inner)
    }
}

impl fmt::Display for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_secret_hex())
    }
}

impl SecretKey {
    /// Parse secp256k1 secret from `bytes`.
    pub fn from_slice(slice: &[u8]) -> Result<Self, Error> {
        Ok(Self::Secp256k1(secp256k1::SecretKey::from_slice(slice)?))
    }

    /// Parse secp256k1 secret from `hex` string.
    pub fn from_hex<S>(hex: S) -> Result<Self, Error>
    where
        S: AsRef<str>,
    {
        Ok(Self::Secp256k1(secp256k1::SecretKey::from_str(
            hex.as_ref(),
        )?))
    }

    /// Derive a BLS scalar by reducing 32-byte input.
    pub fn bls_from_reduced_bytes(bytes: &[u8; 32]) -> Self {
        Self::Bls(BlsSecretKey::from_reduced_bytes(bytes))
    }

    /// Parse BLS scalar from canonical bytes.
    pub fn bls_from_slice(slice: &[u8]) -> Result<Self, Error> {
        Ok(Self::Bls(BlsSecretKey::from_bytes(slice)?))
    }

    /// Generate random secp256k1 secret key.
    pub fn generate() -> Self {
        let (secret_key, _) = SECP256K1.generate_keypair(&mut OsRng);
        Self::Secp256k1(secret_key)
    }

    /// Generate random BLS scalar.
    pub fn generate_bls() -> Self {
        let mut bytes = [0u8; 32];
        OsRng.fill_bytes(&mut bytes);
        Self::bls_from_reduced_bytes(&bytes)
    }

    /// Get secret key as `hex` string.
    pub fn to_secret_hex(&self) -> String {
        crate::util::hex::encode(self.to_secret_bytes())
    }

    /// Get secret key as `bytes`.
    pub fn as_secret_bytes(&self) -> Vec<u8> {
        self.to_secret_bytes().to_vec()
    }

    /// Get secret key as `bytes`.
    pub fn to_secret_bytes(&self) -> [u8; 32] {
        match self {
            Self::Secp256k1(inner) => inner.secret_bytes(),
            Self::Bls(inner) => inner.to_bytes(),
        }
    }

    /// Alias for compatibility with `bitcoin::secp256k1::SecretKey`.
    pub fn secret_bytes(&self) -> [u8; 32] {
        self.to_secret_bytes()
    }

    /// Schnorr Signature on Message.
    pub fn sign(&self, msg: &[u8]) -> Result<Signature, Error> {
        let Self::Secp256k1(inner) = self else {
            return Err(Error::WrongKeyKind);
        };
        let hash: Sha256Hash = Sha256Hash::hash(msg);
        let msg = Message::from_digest_slice(hash.as_ref())?;
        Ok(SECP256K1.sign_schnorr(&msg, &Keypair::from_secret_key(&SECP256K1, inner)))
    }

    /// Get public key.
    pub fn public_key(&self) -> PublicKey {
        match self {
            Self::Secp256k1(inner) => inner.public_key(&SECP256K1).into(),
            Self::Bls(inner) => inner.public_key_g2().into(),
        }
    }

    /// Return secp256k1 x-only public key and parity.
    pub fn x_only_public_key<C>(&self, secp: &Secp256k1<C>) -> (XOnlyPublicKey, secp256k1::Parity)
    where
        C: secp256k1::Signing,
    {
        match self {
            Self::Secp256k1(inner) => inner.x_only_public_key(secp),
            Self::Bls(_) => panic!("BLS scalar is not a secp256k1 key"),
        }
    }

    /// [`SecretKey`] to secp256k1 [`Scalar`].
    #[inline]
    pub fn to_scalar(self) -> Scalar {
        match self {
            Self::Secp256k1(inner) => Scalar::from(inner),
            Self::Bls(_) => panic!("BLS scalar is not a secp256k1 scalar"),
        }
    }

    /// [`SecretKey`] as secp256k1 [`Scalar`].
    #[inline]
    pub fn as_scalar(&self) -> Scalar {
        match self {
            Self::Secp256k1(inner) => Scalar::from(*inner),
            Self::Bls(_) => panic!("BLS scalar is not a secp256k1 scalar"),
        }
    }

    /// Return the secp256k1 secret key.
    pub fn as_secp256k1(&self) -> Result<&secp256k1::SecretKey, Error> {
        match self {
            Self::Secp256k1(inner) => Ok(inner),
            Self::Bls(_) => Err(Error::WrongKeyKind),
        }
    }

    /// Return the BLS scalar.
    pub fn as_bls(&self) -> Result<&BlsSecretKey, Error> {
        match self {
            Self::Bls(inner) => Ok(inner),
            Self::Secp256k1(_) => Err(Error::WrongKeyKind),
        }
    }

    /// Tweak-multiply a secp256k1 secret key.
    pub fn mul_tweak(&self, tweak: &Scalar) -> Result<Self, secp256k1::Error> {
        match self {
            Self::Secp256k1(inner) => Ok(inner.mul_tweak(tweak)?.into()),
            Self::Bls(_) => Err(secp256k1::Error::InvalidSecretKey),
        }
    }

    /// Tweak-add a secp256k1 secret key.
    pub fn add_tweak(&self, tweak: &Scalar) -> Result<Self, secp256k1::Error> {
        match self {
            Self::Secp256k1(inner) => Ok(inner.add_tweak(tweak)?.into()),
            Self::Bls(_) => Err(secp256k1::Error::InvalidSecretKey),
        }
    }

    /// Negate a secp256k1 secret key.
    pub fn negate(&self) -> Self {
        match self {
            Self::Secp256k1(inner) => inner.negate().into(),
            Self::Bls(_) => panic!("cannot negate BLS scalar as a secp256k1 key"),
        }
    }
}

impl FromStr for SecretKey {
    type Err = Error;

    fn from_str(secret_key: &str) -> Result<Self, Self::Err> {
        Self::from_hex(secret_key)
    }
}

impl Serialize for SecretKey {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match serializer.is_human_readable() {
            true => serializer.serialize_str(&self.to_secret_hex()),
            false => serializer.serialize_bytes(&self.to_secret_bytes()),
        }
    }
}

impl<'de> Deserialize<'de> for SecretKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match deserializer.is_human_readable() {
            true => {
                let secret_key: String = String::deserialize(deserializer)?;
                SecretKey::from_hex(secret_key).map_err(serde::de::Error::custom)
            }
            false => {
                struct SecretKeyVisitor;

                impl Visitor<'_> for SecretKeyVisitor {
                    type Value = SecretKey;

                    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                        formatter.write_str("a byte array")
                    }

                    fn visit_bytes<E>(self, value: &[u8]) -> Result<Self::Value, E>
                    where
                        E: serde::de::Error,
                    {
                        SecretKey::from_slice(value).map_err(serde::de::Error::custom)
                    }
                }

                deserializer.deserialize_bytes(SecretKeyVisitor)
            }
        }
    }
}

impl Drop for SecretKey {
    fn drop(&mut self) {
        if let Self::Secp256k1(inner) = self {
            inner.non_secure_erase();
        }
        tracing::trace!("Secret Key dropped.");
    }
}

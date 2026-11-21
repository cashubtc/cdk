use core::fmt;
use core::str::FromStr;

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1;
use bitcoin::secp256k1::rand::rngs::OsRng;
use bitcoin::secp256k1::schnorr::Signature;
use bitcoin::secp256k1::{Keypair, Message, Scalar, XOnlyPublicKey};
use serde::de::Visitor;
use serde::{Deserialize, Deserializer, Serialize};

use super::{Error, PublicKey};
use crate::SECP256K1;

/// Secret key material.
///
/// Secret keys intentionally do not implement [`fmt::Display`]. Export secret
/// material explicitly with [`Self::to_secret_hex`] when it is required by a
/// persistence or interoperability boundary.
///
/// ```compile_fail
/// use cashu::nuts::SecretKey;
///
/// let secret_key = SecretKey::generate();
/// let _ = secret_key.to_string();
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretKey {
    inner: secp256k1::SecretKey,
}

impl From<secp256k1::SecretKey> for SecretKey {
    fn from(inner: secp256k1::SecretKey) -> Self {
        Self { inner }
    }
}

impl SecretKey {
    /// Parse secp256k1 secret from `bytes`.
    pub fn from_slice(slice: &[u8]) -> Result<Self, Error> {
        Ok(Self {
            inner: secp256k1::SecretKey::from_slice(slice)?,
        })
    }

    /// Parse secp256k1 secret from `hex` string.
    pub fn from_hex<S>(hex: S) -> Result<Self, Error>
    where
        S: AsRef<str>,
    {
        Ok(Self {
            inner: secp256k1::SecretKey::from_str(hex.as_ref())?,
        })
    }

    /// Generate random secp256k1 secret key.
    pub fn generate() -> Self {
        let (secret_key, _) = SECP256K1.generate_keypair(&mut OsRng);
        Self { inner: secret_key }
    }

    /// Borrow the underlying secp256k1 secret key.
    pub fn as_secp256k1(&self) -> &secp256k1::SecretKey {
        &self.inner
    }

    /// Get secret key as `hex` string.
    pub fn to_secret_hex(&self) -> String {
        crate::util::hex::encode(self.to_secret_bytes())
    }

    /// Get secret key as `bytes`.
    pub fn as_secret_bytes(&self) -> &[u8] {
        self.inner.as_ref()
    }

    /// Get secret key as `bytes`.
    pub fn to_secret_bytes(&self) -> [u8; 32] {
        self.inner.secret_bytes()
    }

    /// Schnorr Signature on Message.
    pub fn sign(&self, msg: &[u8]) -> Result<Signature, Error> {
        let hash: Sha256Hash = Sha256Hash::hash(msg);
        let msg = Message::from_digest_slice(hash.as_ref())?;
        Ok(SECP256K1.sign_schnorr(&msg, &Keypair::from_secret_key(&SECP256K1, &self.inner)))
    }

    /// Get public key.
    pub fn public_key(&self) -> PublicKey {
        self.inner.public_key(&SECP256K1).into()
    }

    /// Return secp256k1 x-only public key and parity.
    pub fn x_only_public_key(&self) -> (XOnlyPublicKey, secp256k1::Parity) {
        self.inner.x_only_public_key(&SECP256K1)
    }

    /// [`SecretKey`] to secp256k1 [`Scalar`].
    #[inline]
    pub fn to_scalar(self) -> Scalar {
        Scalar::from(self.inner)
    }

    /// [`SecretKey`] as secp256k1 [`Scalar`].
    #[inline]
    pub fn as_scalar(&self) -> Scalar {
        Scalar::from(self.inner)
    }

    /// Tweak-multiply a secp256k1 secret key.
    pub fn mul_tweak(&self, tweak: &Scalar) -> Result<Self, Error> {
        Ok(self.inner.mul_tweak(tweak)?.into())
    }

    /// Tweak-add a secp256k1 secret key.
    pub fn add_tweak(&self, tweak: &Scalar) -> Result<Self, Error> {
        Ok(self.inner.add_tweak(tweak)?.into())
    }

    /// Negate a secp256k1 secret key.
    pub fn negate(&self) -> Self {
        self.inner.negate().into()
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
        if serializer.is_human_readable() {
            serializer.serialize_str(&self.to_secret_hex())
        } else {
            serializer.serialize_bytes(self.as_secret_bytes())
        }
    }
}

impl<'de> Deserialize<'de> for SecretKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if deserializer.is_human_readable() {
            let secret_key = String::deserialize(deserializer)?;
            Self::from_hex(secret_key).map_err(serde::de::Error::custom)
        } else {
            struct SecretKeyVisitor;

            impl Visitor<'_> for SecretKeyVisitor {
                type Value = SecretKey;

                fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                    formatter.write_str("32 secp256k1 secret-key bytes")
                }

                fn visit_bytes<E>(self, value: &[u8]) -> Result<Self::Value, E>
                where
                    E: serde::de::Error,
                {
                    SecretKey::from_slice(value).map_err(E::custom)
                }
            }

            deserializer.deserialize_bytes(SecretKeyVisitor)
        }
    }
}

impl Drop for SecretKey {
    fn drop(&mut self) {
        self.inner.non_secure_erase();
        tracing::trace!("Secret Key dropped.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_preserves_legacy_secp_encoding() {
        let bytes = [7u8; 32];
        let secp = SecretKey::from_slice(&bytes).unwrap();

        assert_eq!(
            serde_json::to_string(&secp).unwrap(),
            format!("\"{}\"", secp.to_secret_hex())
        );
        let json = serde_json::to_string(&secp).unwrap();
        let from_json: SecretKey = serde_json::from_str(&json).unwrap();
        let mut cbor = Vec::new();
        ciborium::into_writer(&secp, &mut cbor).unwrap();
        let from_cbor: SecretKey = ciborium::from_reader(cbor.as_slice()).unwrap();
        assert_eq!(secp, from_json);
        assert_eq!(secp, from_cbor);
        assert_eq!(secp.public_key(), from_cbor.public_key());

        let mut expected_cbor = Vec::new();
        ciborium::into_writer(&serde_bytes::Bytes::new(&bytes), &mut expected_cbor).unwrap();
        assert_eq!(cbor, expected_cbor);
    }

    #[test]
    fn secret_hex_export_is_explicit() {
        let hex = "50d7fd7aa2b2fe4607f41f4ce6f8794fc184dd47b8cdfbe4b3d1249aa02d35aa";
        let secret_key = SecretKey::from_hex(hex).unwrap();

        assert_eq!(secret_key.to_secret_hex(), hex);
    }
}

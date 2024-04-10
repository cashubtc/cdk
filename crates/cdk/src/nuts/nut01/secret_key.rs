use core::fmt;
use core::ops::Deref;
use core::str::FromStr;

use bitcoin::secp256k1;
use bitcoin::secp256k1::rand::rngs::OsRng;
use bitcoin::secp256k1::Scalar;
use serde::{Deserialize, Deserializer, Serialize};

use super::{Error, PublicKey};
use crate::SECP256K1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretKey {
    inner: secp256k1::SecretKey,
}

impl Deref for SecretKey {
    type Target = secp256k1::SecretKey;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<secp256k1::SecretKey> for SecretKey {
    fn from(inner: secp256k1::SecretKey) -> Self {
        Self { inner }
    }
}

impl fmt::Display for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_secret_hex())
    }
}

impl SecretKey {
    /// Parse from `bytes`
    pub fn from_slice(slice: &[u8]) -> Result<Self, Error> {
        Ok(Self {
            inner: secp256k1::SecretKey::from_slice(slice)?,
        })
    }

    /// Parse from `hex` string
    pub fn from_hex<S>(hex: S) -> Result<Self, Error>
    where
        S: AsRef<str>,
    {
        Ok(Self {
            inner: secp256k1::SecretKey::from_str(hex.as_ref())?,
        })
    }

    /// Generate random secret key
    pub fn generate() -> Self {
        let (secret_key, _) = SECP256K1.generate_keypair(&mut OsRng);
        Self { inner: secret_key }
    }

    /// Get secret key as `hex` string
    pub fn to_secret_hex(&self) -> String {
        self.inner.display_secret().to_string()
    }

    /// Get secret key as `bytes`
    pub fn as_secret_bytes(&self) -> &[u8] {
        self.inner.as_ref()
    }

    /// Get secret key as `bytes`
    pub fn to_secret_bytes(&self) -> [u8; 32] {
        self.inner.secret_bytes()
    }

    /// Get public key
    pub fn public_key(&self) -> PublicKey {
        self.inner.public_key(&SECP256K1).into()
    }

    #[inline]
    pub fn to_scalar(self) -> Scalar {
        Scalar::from(self.inner)
    }

    #[inline]
    pub fn as_scalar(&self) -> Scalar {
        Scalar::from(self.inner)
    }
}

impl FromStr for SecretKey {
    type Err = Error;

    /// Try to parse [SecretKey] from `hex` or `bech32`
    fn from_str(secret_key: &str) -> Result<Self, Self::Err> {
        Self::from_hex(secret_key)
    }
}

impl Serialize for SecretKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_secret_hex())
    }
}

impl<'de> Deserialize<'de> for SecretKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let secret_key: String = String::deserialize(deserializer)?;
        Self::from_hex(secret_key).map_err(serde::de::Error::custom)
    }
}

impl Drop for SecretKey {
    fn drop(&mut self) {
        self.inner.non_secure_erase();
        tracing::trace!("Secret Key dropped.");
    }
}

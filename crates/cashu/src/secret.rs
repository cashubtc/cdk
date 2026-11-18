//! Secret

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use bitcoin::secp256k1::rand::{self, RngCore};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::Zeroize;

use crate::nuts::nut01::{PublicKey, SecretKey};
use crate::nuts::nut02::KeySetVersion;
use crate::util::hex;

/// Length in characters of a v3 secret: a hex compressed secp256k1 point.
pub const V3_SECRET_HEX_LEN: usize = 66;

/// The secret data that allows spending ecash
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}

/// Secret Errors
#[derive(Debug, Error)]
pub enum Error {
    /// Invalid Length
    #[error("Invalid secret length: `{0}`")]
    InvalidLength(u64),
    /// Hex Error
    #[error(transparent)]
    Hex(#[from] hex::Error),
    /// Serde Json error
    #[error(transparent)]
    SerdeJsonError(#[from] serde_json::Error),
    /// A v3 secret is not 66 lowercase hex characters encoding a compressed point
    #[error("Invalid v3 secret: expected a lowercase hex compressed secp256k1 point")]
    InvalidV3Secret,
}

impl Default for Secret {
    fn default() -> Self {
        Self::generate()
    }
}

impl Secret {
    /// Create new [`Secret`]
    #[inline]
    pub fn new<S>(secret: S) -> Self
    where
        S: Into<String>,
    {
        Self(secret.into())
    }

    /// Create secret value
    /// Generate a new random secret as the recommended 32 byte hex
    pub fn generate() -> Self {
        let mut rng = rand::thread_rng();

        let mut random_bytes = [0u8; 32];

        // Generate random bytes
        rng.fill_bytes(&mut random_bytes);
        // The secret string is hex encoded
        let secret = hex::encode(random_bytes);
        Self(secret)
    }

    /// Length of the secret string in bytes
    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Check if the secret is empty
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// [`Secret`] as bytes
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    /// [`Secret`] to bytes
    #[inline]
    pub fn to_bytes(&self) -> Vec<u8> {
        self.as_bytes().to_vec()
    }

    /// Create the v3 secret for a point: its compressed encoding as lowercase hex
    pub fn from_point(point: &PublicKey) -> Self {
        Self(point.to_hex())
    }

    /// Generate a random v3 secret, returning the secret key behind it
    ///
    /// The key is the spend key of a bearer v3 proof. Without it the proof
    /// cannot be spent.
    pub fn generate_v3() -> (Self, SecretKey) {
        let secret_key = SecretKey::generate();
        (Self::from_point(&secret_key.public_key()), secret_key)
    }

    /// Generate a random secret of the form a keyset version expects
    ///
    /// Pre-v3 keysets get the usual 32-byte hex string. v3 keysets get a
    /// point; the secret key behind it is returned alongside it.
    pub fn generate_for_version(version: KeySetVersion) -> (Self, Option<SecretKey>) {
        match version {
            KeySetVersion::Version00 | KeySetVersion::Version01 => (Self::generate(), None),
            KeySetVersion::Version02 => {
                let (secret, secret_key) = Self::generate_v3();
                (secret, Some(secret_key))
            }
        }
    }

    /// Decode a v3 secret into its point
    ///
    /// The secret MUST be exactly 66 lowercase hex characters encoding a
    /// compressed secp256k1 point (NUT-00). Any other form is rejected,
    /// including uppercase hex of a valid point.
    pub fn v3_point(&self) -> Result<PublicKey, Error> {
        let bytes = self.v3_bytes()?;
        PublicKey::from_slice(&bytes).map_err(|_| Error::InvalidV3Secret)
    }

    /// Decode a v3 secret into its 33 bytes, after checking it is a point
    pub fn v3_bytes(&self) -> Result<[u8; 33], Error> {
        let s = self.0.as_bytes();
        if s.len() != V3_SECRET_HEX_LEN
            || !s
                .iter()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c))
        {
            return Err(Error::InvalidV3Secret);
        }
        let decoded = hex::decode(&self.0).map_err(|_| Error::InvalidV3Secret)?;
        let bytes: [u8; 33] = decoded
            .as_slice()
            .try_into()
            .map_err(|_| Error::InvalidV3Secret)?;
        if bytes[0] != 0x02 && bytes[0] != 0x03 {
            return Err(Error::InvalidV3Secret);
        }
        PublicKey::from_slice(&bytes).map_err(|_| Error::InvalidV3Secret)?;
        Ok(bytes)
    }

    /// The bytes hashed to the curve for a keyset version
    ///
    /// Pre-v3 keysets hash the UTF-8 bytes of the secret string. v3 keysets
    /// hash the 33 decoded bytes of the point, and reject any secret that is
    /// not one.
    pub fn hash_input(&self, version: KeySetVersion) -> Result<Cow<'_, [u8]>, Error> {
        match version {
            KeySetVersion::Version00 | KeySetVersion::Version01 => {
                Ok(Cow::Borrowed(self.0.as_bytes()))
            }
            KeySetVersion::Version02 => Ok(Cow::Owned(self.v3_bytes()?.to_vec())),
        }
    }

    /// Check if secret is P2PK secret
    pub fn is_p2pk(&self) -> bool {
        use crate::nuts::Kind;

        let secret: Result<crate::nuts::nut10::Secret, serde_json::Error> =
            serde_json::from_str(&self.0);

        if let Ok(secret) = secret {
            if secret.kind().eq(&Kind::P2PK) {
                return true;
            }
        }

        false
    }
}

impl FromStr for Secret {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.to_string()))
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Secret> for Vec<u8> {
    fn from(value: Secret) -> Vec<u8> {
        value.to_bytes()
    }
}

impl From<&Secret> for Vec<u8> {
    fn from(value: &Secret) -> Vec<u8> {
        value.to_bytes()
    }
}

impl TryFrom<Secret> for crate::nuts::nut10::Secret {
    type Error = serde_json::Error;

    fn try_from(unchecked_secret: Secret) -> Result<crate::nuts::nut10::Secret, Self::Error> {
        serde_json::from_str(&unchecked_secret.0)
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl TryFrom<&Secret> for crate::nuts::nut10::Secret {
    type Error = Error;

    fn try_from(unchecked_secret: &Secret) -> Result<crate::nuts::nut10::Secret, Self::Error> {
        Ok(serde_json::from_str(&unchecked_secret.0)?)
    }
}

#[cfg(test)]
mod tests {
    use std::assert_eq;
    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_secret_from_str() {
        let secret = Secret::generate();

        let secret_str = secret.to_string();

        assert_eq!(hex::decode(secret_str.clone()).unwrap().len(), 32);

        let secret_n = Secret::from_str(&secret_str).unwrap();

        assert_eq!(secret_n, secret)
    }

    #[test]
    fn test_is_p2pk_true() {
        // A valid P2PK secret in JSON format
        let p2pk_secret_str = r#"["P2PK",{"nonce":"5d11913ee0f92fefdc82a6764fd2457a","data":"026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198"}]"#;
        let secret = Secret::from_str(p2pk_secret_str).unwrap();

        assert!(secret.is_p2pk());
    }

    #[test]
    fn test_is_p2pk_false() {
        // A random hex string (not a P2PK secret)
        let secret = Secret::generate();

        assert!(!secret.is_p2pk());

        // Also test with a plain string that's not valid JSON
        let plain_secret = Secret::from_str("not_a_p2pk_secret").unwrap();

        assert!(!plain_secret.is_p2pk());
    }

    #[test]
    fn test_secret_to_vec_u8() {
        let secret = Secret::from_str("test_secret_value").unwrap();

        // Test From<Secret> for Vec<u8>
        let bytes: Vec<u8> = secret.clone().into();
        assert_eq!(bytes, b"test_secret_value".to_vec());
        assert!(!bytes.is_empty());

        // Test From<&Secret> for Vec<u8>
        let bytes_ref: Vec<u8> = (&secret).into();
        assert_eq!(bytes_ref, b"test_secret_value".to_vec());
        assert!(!bytes_ref.is_empty());
    }

    #[test]
    fn v3_point_round_trips() {
        let (secret, secret_key) = Secret::generate_v3();
        assert_eq!(secret.len(), V3_SECRET_HEX_LEN);
        assert_eq!(secret.v3_point().unwrap(), secret_key.public_key());
        assert_eq!(
            secret
                .hash_input(KeySetVersion::Version02)
                .unwrap()
                .as_ref(),
            secret_key.public_key().to_bytes().as_slice()
        );
    }

    #[test]
    fn v3_point_rejects_other_forms() {
        let point = "02f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9";
        assert!(Secret::new(point).v3_point().is_ok());

        let rejected = [
            // uppercase hex of a valid point
            point.to_uppercase(),
            // mixed case
            "02F9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9".to_string(),
            // uncompressed prefix with the compressed length
            "04f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9".to_string(),
            // x not on the curve (x = 5)
            format!("02{}", "0".repeat(63) + "5"),
            // too short / too long
            point[..64].to_string(),
            format!("{point}00"),
            // 32-byte hex, the pre-v3 random form
            "a".repeat(64),
            // well-known NUT-10 secret
            r#"["P2PK",{"nonce":"00","data":"02f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9"}]"#.to_string(),
            String::new(),
        ];
        for s in rejected {
            assert!(
                matches!(
                    Secret::new(s.clone()).v3_point(),
                    Err(Error::InvalidV3Secret)
                ),
                "accepted {s}"
            );
            assert!(Secret::new(s).hash_input(KeySetVersion::Version02).is_err());
        }
    }

    #[test]
    fn pre_v3_hash_input_is_utf8() {
        let secret = Secret::new("not a point");
        for version in [KeySetVersion::Version00, KeySetVersion::Version01] {
            assert_eq!(
                secret.hash_input(version).unwrap().as_ref(),
                b"not a point".as_slice()
            );
        }
    }

    #[test]
    fn debug_redacts_secret_value() {
        let secret_value = "spendable-proof-secret";
        let secret = Secret::new(secret_value);

        let debug = format!("{secret:?}");

        assert_eq!(debug, "Secret([REDACTED])");
        assert!(!debug.contains(secret_value));
    }
}

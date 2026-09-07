use core::fmt;
use core::str::FromStr;

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1;
use bitcoin::secp256k1::rand::rngs::OsRng;
use bitcoin::secp256k1::schnorr::Signature;
use bitcoin::secp256k1::{Keypair, Message, Scalar, Secp256k1, XOnlyPublicKey};
use serde::de::Visitor;
use serde::{Deserialize, Deserializer, Serialize};

use super::{BlsSecretKey, Error, PublicKey};
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
    ///
    /// Uses rejection sampling so the scalar is uniform over `Fr*`: a canonical
    /// non-zero value strictly below the field order. Modular reduction of raw
    /// bytes would bias the distribution and could yield the forbidden zero
    /// blinding factor.
    pub fn generate_bls() -> Self {
        Self::Bls(BlsSecretKey::generate())
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

    /// Deterministic Schnorr Signature on Message without auxiliary randomness.
    ///
    /// Prefer [`Self::sign`] for normal operation. This helper is intended for
    /// reproducible fixtures where byte-for-byte stable signatures are needed.
    pub fn sign_no_aux_rand(&self, msg: &[u8]) -> Result<Signature, Error> {
        let Self::Secp256k1(inner) = self else {
            return Err(Error::WrongKeyKind);
        };
        let hash: Sha256Hash = Sha256Hash::hash(msg);
        let msg = Message::from_digest_slice(hash.as_ref())?;
        Ok(SECP256K1.sign_schnorr_no_aux_rand(&msg, &Keypair::from_secret_key(&SECP256K1, inner)))
    }

    /// Get public key.
    pub fn public_key(&self) -> PublicKey {
        match self {
            Self::Secp256k1(inner) => inner.public_key(&SECP256K1).into(),
            Self::Bls(inner) => inner.public_key_g2().into(),
        }
    }

    /// Return secp256k1 x-only public key and parity.
    ///
    /// Returns [`Error::WrongKeyKind`] for BLS secret keys.
    pub fn try_x_only_public_key<C>(
        &self,
        secp: &Secp256k1<C>,
    ) -> Result<(XOnlyPublicKey, secp256k1::Parity), Error>
    where
        C: secp256k1::Signing,
    {
        match self {
            Self::Secp256k1(inner) => Ok(inner.x_only_public_key(secp)),
            Self::Bls(_) => Err(Error::WrongKeyKind),
        }
    }

    /// [`SecretKey`] to secp256k1 [`Scalar`].
    ///
    /// Returns [`Error::WrongKeyKind`] for BLS secret keys.
    #[inline]
    pub fn try_to_scalar(self) -> Result<Scalar, Error> {
        match self {
            Self::Secp256k1(inner) => Ok(Scalar::from(inner)),
            Self::Bls(_) => Err(Error::WrongKeyKind),
        }
    }

    /// [`SecretKey`] as secp256k1 [`Scalar`].
    ///
    /// Returns [`Error::WrongKeyKind`] for BLS secret keys.
    #[inline]
    pub fn try_as_scalar(&self) -> Result<Scalar, Error> {
        match self {
            Self::Secp256k1(inner) => Ok(Scalar::from(*inner)),
            Self::Bls(_) => Err(Error::WrongKeyKind),
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
    pub fn try_mul_tweak(&self, tweak: &Scalar) -> Result<Self, secp256k1::Error> {
        match self {
            Self::Secp256k1(inner) => Ok(inner.mul_tweak(tweak)?.into()),
            Self::Bls(_) => Err(secp256k1::Error::InvalidSecretKey),
        }
    }

    /// Tweak-add a secp256k1 secret key.
    pub fn try_add_tweak(&self, tweak: &Scalar) -> Result<Self, secp256k1::Error> {
        match self {
            Self::Secp256k1(inner) => Ok(inner.add_tweak(tweak)?.into()),
            Self::Bls(_) => Err(secp256k1::Error::InvalidSecretKey),
        }
    }

    /// Negate a secp256k1 secret key.
    ///
    /// Returns [`Error::WrongKeyKind`] for BLS secret keys.
    pub fn try_negate(&self) -> Result<Self, Error> {
        match self {
            Self::Secp256k1(inner) => Ok(inner.negate().into()),
            Self::Bls(_) => Err(Error::WrongKeyKind),
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
            true => match self {
                Self::Secp256k1(_) => serializer.serialize_str(&self.to_secret_hex()),
                Self::Bls(_) => serializer.serialize_str(&format!("bls:{}", self.to_secret_hex())),
            },
            false => match self {
                Self::Secp256k1(_) => serializer.serialize_bytes(&self.to_secret_bytes()),
                Self::Bls(_) => {
                    let mut tagged = [0_u8; 33];
                    tagged[0] = 1;
                    tagged[1..].copy_from_slice(&self.to_secret_bytes());
                    serializer.serialize_bytes(&tagged)
                }
            },
        }
    }
}

impl<'de> Deserialize<'de> for SecretKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match deserializer.is_human_readable() {
            true => {
                let secret_key: String = String::deserialize(deserializer)?;
                match secret_key.strip_prefix("bls:") {
                    Some(secret_key) => {
                        let bytes = crate::util::hex::decode(secret_key)
                            .map_err(serde::de::Error::custom)?;
                        SecretKey::bls_from_slice(&bytes).map_err(serde::de::Error::custom)
                    }
                    None => SecretKey::from_hex(secret_key).map_err(serde::de::Error::custom),
                }
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
                        match value {
                            [1, secret_key @ ..] if secret_key.len() == 32 => {
                                SecretKey::bls_from_slice(secret_key).map_err(E::custom)
                            }
                            _ => SecretKey::from_slice(value).map_err(E::custom),
                        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_bls_is_canonical_and_non_zero() {
        for _ in 0..256 {
            let key = SecretKey::generate_bls();
            let bytes = key.to_secret_bytes();
            // Non-zero.
            assert_ne!(bytes, [0u8; 32]);
            // Canonical: re-parsing as a canonical BLS scalar must succeed and round-trip.
            let reparsed = SecretKey::bls_from_slice(&bytes).expect("canonical scalar");
            assert_eq!(reparsed.to_secret_bytes(), bytes);
        }
    }

    #[test]
    fn secret_hex_export_is_explicit() {
        let hex = "50d7fd7aa2b2fe4607f41f4ce6f8794fc184dd47b8cdfbe4b3d1249aa02d35aa";
        let secret_key = SecretKey::from_hex(hex).unwrap();

        assert_eq!(secret_key.to_secret_hex(), hex);
    }

    #[test]
    fn secp_only_operations_reject_bls_secret_keys() {
        let bls = SecretKey::bls_from_reduced_bytes(&[9; 32]);

        assert!(matches!(
            bls.try_x_only_public_key(&SECP256K1),
            Err(Error::WrongKeyKind)
        ));
        assert!(matches!(
            bls.clone().try_to_scalar(),
            Err(Error::WrongKeyKind)
        ));
        assert!(matches!(bls.try_as_scalar(), Err(Error::WrongKeyKind)));
        assert!(matches!(bls.try_negate(), Err(Error::WrongKeyKind)));
    }

    #[test]
    fn secp_and_bls_secret_key_serialization_round_trip() {
        let secp_hex = "50d7fd7aa2b2fe4607f41f4ce6f8794fc184dd47b8cdfbe4b3d1249aa02d35aa";
        let secp = SecretKey::from_hex(secp_hex).expect("secp key");
        let secp_json = serde_json::to_string(&secp).expect("serialize secp key");
        assert_eq!(secp_json, format!("\"{secp_hex}\""));
        assert_eq!(
            serde_json::from_str::<SecretKey>(&secp_json).expect("deserialize secp key"),
            secp
        );

        let bls = SecretKey::bls_from_reduced_bytes(&[11; 32]);
        let bls_json = serde_json::to_string(&bls).expect("serialize BLS key");
        assert!(bls_json.starts_with("\"bls:"));
        assert_eq!(
            serde_json::from_str::<SecretKey>(&bls_json).expect("deserialize BLS key"),
            bls
        );

        for key in [secp, bls] {
            let mut encoded = Vec::new();
            ciborium::into_writer(&key, &mut encoded).expect("serialize binary key");
            let decoded: SecretKey =
                ciborium::from_reader(encoded.as_slice()).expect("deserialize binary key");
            assert_eq!(decoded, key);
        }
    }
}

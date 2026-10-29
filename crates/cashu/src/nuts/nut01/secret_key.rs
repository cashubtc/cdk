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

/// Secret key material.
///
/// Secret keys intentionally do not implement [`fmt::Display`]. Export secret
/// material explicitly with [`Self::to_secret_hex`] when it is required by a
/// persistence or interoperability boundary.
///
/// Serde preserves legacy secp256k1 encodings (hex strings or 32 raw bytes).
/// BLS scalars use a `bls:` prefix in human-readable formats and a `0x02`
/// tag followed by 32 scalar bytes in binary formats. These tags identify
/// locally serialized key material; raw protocol exports remain untagged.
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
        loop {
            let mut bytes = [0u8; 32];
            OsRng.fill_bytes(&mut bytes);
            // `bls_from_slice` only succeeds when the value is canonical (< order);
            // reject the all-zero scalar so `r` is always in `Fr*`.
            if bytes != [0u8; 32] {
                if let Ok(secret_key) = Self::bls_from_slice(&bytes) {
                    return secret_key;
                }
            }
        }
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
        match (self, serializer.is_human_readable()) {
            (Self::Secp256k1(_), true) => serializer.serialize_str(&self.to_secret_hex()),
            (Self::Secp256k1(_), false) => serializer.serialize_bytes(&self.to_secret_bytes()),
            (Self::Bls(_), true) => {
                serializer.serialize_str(&format!("bls:{}", self.to_secret_hex()))
            }
            (Self::Bls(_), false) => {
                let mut tagged = [0u8; 33];
                tagged[0] = 0x02;
                tagged[1..].copy_from_slice(&self.to_secret_bytes());
                serializer.serialize_bytes(&tagged)
            }
        }
    }
}

impl<'de> Deserialize<'de> for SecretKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match deserializer.is_human_readable() {
            true => {
                let secret_key: String = String::deserialize(deserializer)?;
                match secret_key.strip_prefix("bls:") {
                    Some(hex) => {
                        let bytes =
                            crate::util::hex::decode(hex).map_err(serde::de::Error::custom)?;
                        Self::bls_from_slice(&bytes).map_err(serde::de::Error::custom)
                    }
                    None => Self::from_hex(secret_key).map_err(serde::de::Error::custom),
                }
            }
            false => {
                struct SecretKeyVisitor;

                impl Visitor<'_> for SecretKeyVisitor {
                    type Value = SecretKey;

                    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                        formatter.write_str("32 secp256k1 bytes or 0x02 followed by 32 BLS bytes")
                    }

                    fn visit_bytes<E>(self, value: &[u8]) -> Result<Self::Value, E>
                    where
                        E: serde::de::Error,
                    {
                        match value {
                            [0x02, scalar @ ..] if scalar.len() == 32 => {
                                SecretKey::bls_from_slice(scalar).map_err(serde::de::Error::custom)
                            }
                            _ => SecretKey::from_slice(value).map_err(serde::de::Error::custom),
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
        // The BLS variant is zeroized by BlsSecretKey::drop when fields are dropped.
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
    fn serde_preserves_key_kind_and_legacy_secp_encoding() {
        let bytes = [7u8; 32];
        let secp = SecretKey::from_slice(&bytes).unwrap();
        let bls = SecretKey::bls_from_slice(&bytes).unwrap();

        assert_eq!(
            serde_json::to_string(&secp).unwrap(),
            format!("\"{}\"", secp.to_secret_hex())
        );
        assert_eq!(
            serde_json::to_string(&bls).unwrap(),
            format!("\"bls:{}\"", bls.to_secret_hex())
        );

        for key in [secp, bls] {
            let json = serde_json::to_string(&key).unwrap();
            let from_json: SecretKey = serde_json::from_str(&json).unwrap();
            let mut cbor = Vec::new();
            ciborium::into_writer(&key, &mut cbor).unwrap();
            let from_cbor: SecretKey = ciborium::from_reader(cbor.as_slice()).unwrap();
            assert_eq!(key, from_json);
            assert_eq!(key, from_cbor);
            assert_eq!(key.public_key(), from_cbor.public_key());

            let expected_bytes = match key {
                SecretKey::Secp256k1(_) => bytes.to_vec(),
                SecretKey::Bls(_) => [&[0x02][..], &bytes].concat(),
            };
            let mut expected_cbor = Vec::new();
            ciborium::into_writer(
                &serde_bytes::Bytes::new(&expected_bytes),
                &mut expected_cbor,
            )
            .unwrap();
            assert_eq!(cbor, expected_cbor);
        }
    }

    #[test]
    fn restored_bls_blinding_factor_can_unblind_signature() {
        use crate::dhke::{blind_message_for_version, sign_message, verify_bls_message};
        use crate::nuts::KeySetVersion;

        let message = b"persisted blinding factor";
        let (blinded, r) =
            blind_message_for_version(message, None, KeySetVersion::Version02).unwrap();
        let mint_key = SecretKey::generate_bls();
        let signature = sign_message(&mint_key, &blinded).unwrap();
        let stored = serde_json::to_vec(&r).unwrap();
        let restored: SecretKey = serde_json::from_slice(&stored).unwrap();
        let unblinded = signature
            .as_bls_g1()
            .unwrap()
            .mul(&restored.as_bls().unwrap().invert().unwrap());
        verify_bls_message(mint_key.public_key(), unblinded.into(), message).unwrap();
    }

    #[test]
    fn serde_rejects_invalid_bls_scalars_and_tags() {
        for text in [
            "bls:01".to_string(),
            format!("bls:{}", "ff".repeat(32)),
            format!("unknown:{}", "01".repeat(32)),
        ] {
            assert!(
                serde_json::from_str::<SecretKey>(&serde_json::to_string(&text).unwrap()).is_err()
            );
        }
        for bytes in [
            vec![0x02; 32 + 2],
            [&[0x03][..], &[1u8; 32]].concat(),
            [&[0x02][..], &[0xff; 32]].concat(),
        ] {
            let mut cbor = Vec::new();
            ciborium::into_writer(&serde_bytes::Bytes::new(&bytes), &mut cbor).unwrap();
            assert!(ciborium::from_reader::<SecretKey, _>(cbor.as_slice()).is_err());
        }
    }

    #[test]
    fn secret_hex_export_is_explicit() {
        let hex = "50d7fd7aa2b2fe4607f41f4ce6f8794fc184dd47b8cdfbe4b3d1249aa02d35aa";
        let secret_key = SecretKey::from_hex(hex).unwrap();

        assert_eq!(secret_key.to_secret_hex(), hex);
    }

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
}

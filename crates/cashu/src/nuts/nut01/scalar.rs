use core::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};

use super::{BlsSecretKey, Error, MintPublicKey, SecretKey};

/// A scalar used to blind a Cashu message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlindingFactor {
    /// Secp256k1 scalar used by v1 and v2 keysets.
    Secp256k1(SecretKey),
    /// BLS12-381 scalar used by v3 keysets.
    Bls(BlsSecretKey),
}

impl BlindingFactor {
    /// Parse a secp256k1 scalar.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, Error> {
        Ok(Self::Secp256k1(SecretKey::from_slice(bytes)?))
    }

    /// Parse a secp256k1 scalar from hex.
    pub fn from_hex<S>(hex: S) -> Result<Self, Error>
    where
        S: AsRef<str>,
    {
        Ok(Self::Secp256k1(SecretKey::from_hex(hex)?))
    }

    /// Parse a canonical non-zero BLS scalar.
    pub fn bls_from_slice(bytes: &[u8]) -> Result<Self, Error> {
        Ok(Self::Bls(BlsSecretKey::from_bytes(bytes)?))
    }

    /// Derive a non-zero BLS scalar by reducing bytes.
    pub fn bls_from_reduced_bytes(bytes: &[u8; 32]) -> Result<Self, Error> {
        Ok(Self::Bls(BlsSecretKey::from_reduced_bytes(bytes)?))
    }

    /// Generate a random secp256k1 scalar.
    pub fn generate() -> Self {
        Self::Secp256k1(SecretKey::generate())
    }

    /// Generate a random BLS scalar.
    pub fn generate_bls() -> Result<Self, Error> {
        Ok(Self::Bls(BlsSecretKey::generate()?))
    }

    /// Export the scalar bytes.
    pub fn to_secret_bytes(&self) -> [u8; 32] {
        match self {
            Self::Secp256k1(key) => key.to_secret_bytes(),
            Self::Bls(key) => key.to_bytes(),
        }
    }

    /// Alias for compatibility with secp256k1 scalar APIs.
    pub fn secret_bytes(&self) -> [u8; 32] {
        self.to_secret_bytes()
    }

    /// Export the scalar bytes as hex.
    pub fn to_secret_hex(&self) -> String {
        crate::util::hex::encode(self.to_secret_bytes())
    }

    /// Return the secp256k1 scalar.
    pub fn as_secp256k1(&self) -> Result<&SecretKey, Error> {
        match self {
            Self::Secp256k1(key) => Ok(key),
            Self::Bls(_) => Err(Error::WrongKeyKind),
        }
    }

    /// Return the BLS scalar.
    pub fn as_bls(&self) -> Result<&BlsSecretKey, Error> {
        match self {
            Self::Bls(key) => Ok(key),
            Self::Secp256k1(_) => Err(Error::WrongKeyKind),
        }
    }

    /// Derive the secp256k1 blinding point.
    pub fn secp_public_key(&self) -> Result<super::PublicKey, Error> {
        Ok(self.as_secp256k1()?.public_key())
    }
}

impl From<SecretKey> for BlindingFactor {
    fn from(key: SecretKey) -> Self {
        Self::Secp256k1(key)
    }
}

impl From<bitcoin::secp256k1::SecretKey> for BlindingFactor {
    fn from(key: bitcoin::secp256k1::SecretKey) -> Self {
        Self::Secp256k1(key.into())
    }
}

impl From<BlsSecretKey> for BlindingFactor {
    fn from(key: BlsSecretKey) -> Self {
        Self::Bls(key)
    }
}

impl FromStr for BlindingFactor {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::from_hex(value)
    }
}

impl Serialize for BlindingFactor {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_scalar(self.to_secret_bytes(), serializer)
    }
}

/// Deserializes a secp256k1 scalar only.
///
/// The encoding has no curve tag, and the only deserialized scalars are
/// legacy ones (NUT-12 DLEQ blinding factors), so a BLS scalar must be
/// rebuilt with [`Self::bls_from_slice`] using its keyset version.
impl<'de> Deserialize<'de> for BlindingFactor {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::Secp256k1(SecretKey::deserialize(deserializer)?))
    }
}

/// A mint signing scalar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MintSecretKey {
    /// Secp256k1 scalar used by v1 and v2 keysets.
    Secp256k1(SecretKey),
    /// BLS12-381 scalar used by v3 keysets.
    Bls(BlsSecretKey),
}

impl MintSecretKey {
    /// Parse a secp256k1 scalar.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, Error> {
        Ok(Self::Secp256k1(SecretKey::from_slice(bytes)?))
    }

    /// Parse a secp256k1 scalar from hex.
    pub fn from_hex<S>(hex: S) -> Result<Self, Error>
    where
        S: AsRef<str>,
    {
        Ok(Self::Secp256k1(SecretKey::from_hex(hex)?))
    }

    /// Parse a canonical non-zero BLS scalar.
    pub fn bls_from_slice(bytes: &[u8]) -> Result<Self, Error> {
        Ok(Self::Bls(BlsSecretKey::from_bytes(bytes)?))
    }

    /// Derive a non-zero BLS scalar by reducing bytes.
    pub fn bls_from_reduced_bytes(bytes: &[u8; 32]) -> Result<Self, Error> {
        Ok(Self::Bls(BlsSecretKey::from_reduced_bytes(bytes)?))
    }

    /// Generate a random secp256k1 scalar.
    pub fn generate() -> Self {
        Self::Secp256k1(SecretKey::generate())
    }

    /// Generate a random BLS scalar.
    pub fn generate_bls() -> Result<Self, Error> {
        Ok(Self::Bls(BlsSecretKey::generate()?))
    }

    /// Export the scalar bytes.
    pub fn to_secret_bytes(&self) -> [u8; 32] {
        match self {
            Self::Secp256k1(key) => key.to_secret_bytes(),
            Self::Bls(key) => key.to_bytes(),
        }
    }

    /// Alias for compatibility with secp256k1 scalar APIs.
    pub fn secret_bytes(&self) -> [u8; 32] {
        self.to_secret_bytes()
    }

    /// Export the scalar bytes as hex.
    pub fn to_secret_hex(&self) -> String {
        crate::util::hex::encode(self.to_secret_bytes())
    }

    /// Return the secp256k1 scalar.
    pub fn as_secp256k1(&self) -> Result<&SecretKey, Error> {
        match self {
            Self::Secp256k1(key) => Ok(key),
            Self::Bls(_) => Err(Error::WrongKeyKind),
        }
    }

    /// Return the BLS scalar.
    pub fn as_bls(&self) -> Result<&BlsSecretKey, Error> {
        match self {
            Self::Bls(key) => Ok(key),
            Self::Secp256k1(_) => Err(Error::WrongKeyKind),
        }
    }

    /// Derive the corresponding mint public key.
    pub fn public_key(&self) -> MintPublicKey {
        match self {
            Self::Secp256k1(key) => key.public_key().into(),
            Self::Bls(key) => key.public_key_g2().into(),
        }
    }
}

impl From<SecretKey> for MintSecretKey {
    fn from(key: SecretKey) -> Self {
        Self::Secp256k1(key)
    }
}

impl From<bitcoin::secp256k1::SecretKey> for MintSecretKey {
    fn from(key: bitcoin::secp256k1::SecretKey) -> Self {
        Self::Secp256k1(key.into())
    }
}

impl From<BlsSecretKey> for MintSecretKey {
    fn from(key: BlsSecretKey) -> Self {
        Self::Bls(key)
    }
}

impl FromStr for MintSecretKey {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::from_hex(value)
    }
}

impl Serialize for MintSecretKey {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_scalar(self.to_secret_bytes(), serializer)
    }
}

/// Deserializes a secp256k1 scalar only.
///
/// The encoding has no curve tag, and the only deserialized scalars are
/// legacy ones (NUT-12 DLEQ blinding factors), so a BLS scalar must be
/// rebuilt with [`Self::bls_from_slice`] using its keyset version.
impl<'de> Deserialize<'de> for MintSecretKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::Secp256k1(SecretKey::deserialize(deserializer)?))
    }
}

/// Serialize scalar bytes the way [`SecretKey`] does: hex in human-readable
/// formats and raw bytes otherwise.
///
/// The encoding carries no curve tag. Every serialized type holding a BLS
/// scalar also carries its keyset id, which identifies the curve.
fn serialize_scalar<S>(bytes: [u8; 32], serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match serializer.is_human_readable() {
        true => serializer.serialize_str(&crate::util::hex::encode(bytes)),
        false => serializer.serialize_bytes(&bytes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalars_serialize_as_plain_bytes_for_both_curves() {
        let secp = BlindingFactor::from_slice(&[7u8; 32]).unwrap();
        let bls = BlindingFactor::bls_from_slice(&[7u8; 32]).unwrap();
        let expected_json = format!("\"{}\"", "07".repeat(32));

        for scalar in [&secp, &bls] {
            assert_eq!(serde_json::to_string(scalar).unwrap(), expected_json);
            let mut cbor = Vec::new();
            ciborium::into_writer(scalar, &mut cbor).unwrap();
            let mut expected_cbor = Vec::new();
            ciborium::into_writer(&serde_bytes::Bytes::new(&[7u8; 32]), &mut expected_cbor)
                .unwrap();
            assert_eq!(cbor, expected_cbor);
        }

        let mint_bls = MintSecretKey::bls_from_slice(&[7u8; 32]).unwrap();
        assert_eq!(serde_json::to_string(&mint_bls).unwrap(), expected_json);
    }

    #[test]
    fn scalars_deserialize_as_secp256k1() {
        let json = format!("\"{}\"", "07".repeat(32));
        assert!(matches!(
            serde_json::from_str::<BlindingFactor>(&json).unwrap(),
            BlindingFactor::Secp256k1(_)
        ));
        assert!(matches!(
            serde_json::from_str::<MintSecretKey>(&json).unwrap(),
            MintSecretKey::Secp256k1(_)
        ));

        // The removed curve-tagged encodings are no longer accepted.
        let tagged = format!("\"bls:{}\"", "07".repeat(32));
        assert!(serde_json::from_str::<BlindingFactor>(&tagged).is_err());
        let mut tagged_bytes = vec![0x02];
        tagged_bytes.extend([7u8; 32]);
        let mut cbor = Vec::new();
        ciborium::into_writer(&serde_bytes::Bytes::new(&tagged_bytes), &mut cbor).unwrap();
        assert!(ciborium::from_reader::<BlindingFactor, _>(cbor.as_slice()).is_err());
    }
}

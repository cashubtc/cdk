use core::fmt;
use core::hash::{Hash, Hasher};
use core::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};

use super::{bls, BlsG1PublicKey, DhkePoint, Error, PublicKey};

const SECP256K1_POINT_SIZE: usize = 33;
const BLS_G1_POINT_SIZE: usize = 48;

/// Serialized `Y` identifying a proof.
///
/// A proof identifier is used for equality and database lookup. Parsing it does
/// not perform elliptic-curve or subgroup validation.
///
/// `Y` is computed with the hash-to-curve function of the proof's keyset
/// (NUT-07), so one secret has a different `ProofId` on each curve: 33 bytes on
/// secp256k1 keysets and 48 bytes on BLS12-381 (v3) keysets. Spent state is
/// tracked per curve. A secret signed under a keyset on each curve can be spent
/// once on each; this is sound because each proof needed its own blind
/// signature, and so its own paid-for output.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ProofId {
    bytes: [u8; BLS_G1_POINT_SIZE],
    len: u8,
}

impl ProofId {
    fn from_supported_slice(bytes: &[u8]) -> Self {
        let mut encoded = [0u8; BLS_G1_POINT_SIZE];
        encoded[..bytes.len()].copy_from_slice(bytes);
        Self {
            bytes: encoded,
            len: bytes.len() as u8,
        }
    }

    /// Parse a compressed proof identifier without curve validation.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, Error> {
        if !matches!(bytes.len(), SECP256K1_POINT_SIZE | BLS_G1_POINT_SIZE) {
            return Err(Error::InvalidPublicKeySize {
                expected: SECP256K1_POINT_SIZE,
                found: bytes.len(),
            });
        }

        Ok(Self::from_supported_slice(bytes))
    }

    /// Parse a hexadecimal proof identifier without curve validation.
    ///
    /// A BLS12-381 identifier must be lowercase hex (NUT-00).
    pub fn from_hex<S>(hex: S) -> Result<Self, Error>
    where
        S: AsRef<str>,
    {
        let hex = hex.as_ref();
        let bytes = if hex.len() == BLS_G1_POINT_SIZE * 2 {
            bls::decode_point_hex(hex)?
        } else {
            crate::util::hex::decode(hex)?
        };
        Self::from_slice(&bytes)
    }

    /// Return the compressed identifier bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }

    /// Return the compressed identifier bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.as_bytes().to_vec()
    }

    /// Return the identifier as hexadecimal.
    pub fn to_hex(&self) -> String {
        crate::util::hex::encode(self.as_bytes())
    }

    /// Whether this identifies a v3 proof
    ///
    /// v3 keysets hash to BLS12-381 G1, so their `Y` has the BLS point size.
    pub fn is_v3(&self) -> bool {
        usize::from(self.len) == BLS_G1_POINT_SIZE
    }
}

impl From<DhkePoint> for ProofId {
    fn from(point: DhkePoint) -> Self {
        match point {
            DhkePoint::Secp256k1(point) => Self::from(point),
            DhkePoint::Bls(point) => Self::from(point),
        }
    }
}

impl From<PublicKey> for ProofId {
    fn from(point: PublicKey) -> Self {
        Self::from_supported_slice(&point.to_bytes())
    }
}

impl From<BlsG1PublicKey> for ProofId {
    fn from(point: BlsG1PublicKey) -> Self {
        Self::from_supported_slice(&point.to_bytes())
    }
}

impl Hash for ProofId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_bytes().hash(state);
    }
}

impl PartialOrd for ProofId {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ProofId {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.as_bytes().cmp(other.as_bytes())
    }
}

impl fmt::Debug for ProofId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProofId({})", self.to_hex())
    }
}

impl fmt::Display for ProofId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for ProofId {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::from_hex(value)
    }
}

impl Serialize for ProofId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for ProofId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        Self::from_hex(encoded).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_identifier_bytes_without_curve_validation() {
        let invalid_secp_point = [0xff; SECP256K1_POINT_SIZE];
        let invalid_bls_point = [0xff; BLS_G1_POINT_SIZE];

        assert!(ProofId::from_slice(&invalid_secp_point).is_ok());
        assert!(ProofId::from_slice(&invalid_bls_point).is_ok());
        assert!(DhkePoint::from_slice_any_curve(&invalid_secp_point).is_err());
        assert!(DhkePoint::from_slice_any_curve(&invalid_bls_point).is_err());
    }

    #[test]
    fn from_hex_requires_lowercase_bls_identifiers() {
        let bls_hex = "ab".repeat(BLS_G1_POINT_SIZE);
        let secp_hex = "ab".repeat(SECP256K1_POINT_SIZE);

        assert!(ProofId::from_hex(&bls_hex).is_ok());
        assert!(ProofId::from_hex(bls_hex.to_uppercase()).is_err());
        assert!(
            serde_json::from_str::<ProofId>(&format!("\"{}\"", bls_hex.to_uppercase())).is_err()
        );
        // secp256k1 identifiers stay case-insensitive.
        assert_eq!(
            ProofId::from_hex(secp_hex.to_uppercase()).expect("uppercase secp256k1"),
            ProofId::from_hex(&secp_hex).expect("lowercase secp256k1"),
        );
    }

    #[test]
    fn is_v3_follows_the_curve() {
        let secp = ProofId::from_slice(&[0x02; SECP256K1_POINT_SIZE]).expect("secp id");
        let bls = ProofId::from_slice(&[0x02; BLS_G1_POINT_SIZE]).expect("bls id");

        assert!(!secp.is_v3());
        assert!(bls.is_v3());
    }
}

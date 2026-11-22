use core::fmt;
use core::hash::{Hash, Hasher};

use serde::Serialize;

use super::{Error, PublicKey};
use crate::nuts::nut02::KeySetVersion;

/// A point used by Cashu's blind-signature protocol: `B_`, `C_` or `C`.
///
/// The curve is selected by the keyset version.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DhkePoint {
    /// Secp256k1 point used by v1 and v2 keysets.
    Secp256k1(PublicKey),
}

impl DhkePoint {
    /// Parse a point using the curve selected by a keyset version.
    pub fn from_slice_for_version(bytes: &[u8], version: KeySetVersion) -> Result<Self, Error> {
        match version {
            KeySetVersion::Version00 | KeySetVersion::Version01 => {
                if bytes.len() != 33 {
                    return Err(Error::InvalidPublicKeySize {
                        expected: 33,
                        found: bytes.len(),
                    });
                }
                Ok(Self::Secp256k1(PublicKey::from_slice(bytes)?))
            }
        }
    }

    /// Parse a hexadecimal point using the curve selected by a keyset version.
    pub fn from_hex_for_version<S>(hex: S, version: KeySetVersion) -> Result<Self, Error>
    where
        S: AsRef<str>,
    {
        Self::from_slice_for_version(&crate::util::hex::decode(hex.as_ref())?, version)
    }

    /// Whether this point is on the curve used by `version`.
    pub fn is_for_version(&self, version: KeySetVersion) -> bool {
        match version {
            KeySetVersion::Version00 | KeySetVersion::Version01 => {
                matches!(self, Self::Secp256k1(_))
            }
        }
    }

    /// Parse a point, inferring the curve from the encoding length.
    ///
    /// Only for trusted storage that does not keep the keyset version next to
    /// the point. Untrusted input must use [`Self::from_slice_for_version`],
    /// which selects the curve before decoding and rejects the wrong one.
    pub fn from_slice_any_curve(bytes: &[u8]) -> Result<Self, Error> {
        match bytes.len() {
            33 | 65 => Ok(Self::Secp256k1(PublicKey::from_slice(bytes)?)),
            found => Err(Error::InvalidPublicKeySize {
                expected: 33,
                found,
            }),
        }
    }

    /// Parse a hexadecimal point, inferring the curve from the encoding length.
    ///
    /// Only for trusted storage; see [`Self::from_slice_any_curve`].
    pub fn from_hex_any_curve<S>(hex: S) -> Result<Self, Error>
    where
        S: AsRef<str>,
    {
        let bytes = crate::util::hex::decode(hex.as_ref())?;
        if bytes.len() == 65 {
            return Err(Error::InvalidPublicKeySize {
                expected: 33,
                found: 65,
            });
        }
        Self::from_slice_any_curve(&bytes)
    }

    /// Return the compressed encoding.
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            Self::Secp256k1(point) => point.to_bytes().to_vec(),
        }
    }

    /// Return the encoded point as hex.
    pub fn to_hex(&self) -> String {
        crate::util::hex::encode(self.to_bytes())
    }

    /// Return the secp256k1 point.
    pub fn as_secp256k1(&self) -> Result<PublicKey, Error> {
        match self {
            Self::Secp256k1(point) => Ok(*point),
        }
    }
}

impl From<PublicKey> for DhkePoint {
    fn from(point: PublicKey) -> Self {
        Self::Secp256k1(point)
    }
}

impl Hash for DhkePoint {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            Self::Secp256k1(point) => point.to_bytes().as_slice().hash(state),
        }
    }
}

impl PartialOrd for DhkePoint {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DhkePoint {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        match (self, other) {
            (Self::Secp256k1(left), Self::Secp256k1(right)) => {
                left.to_bytes().cmp(&right.to_bytes())
            }
        }
    }
}

impl fmt::Debug for DhkePoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DhkePoint({})", self.to_hex())
    }
}

impl fmt::Display for DhkePoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl Serialize for DhkePoint {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_for_version_matches_curve_to_keyset_version() {
        let secp: DhkePoint = crate::nuts::SecretKey::generate().public_key().into();
        for version in [KeySetVersion::Version00, KeySetVersion::Version01] {
            assert!(secp.is_for_version(version));
        }
    }

    #[test]
    fn version_selects_curve_before_point_decoding() {
        let wrong_sized = [0xff; 48];

        assert!(matches!(
            DhkePoint::from_slice_for_version(&wrong_sized, KeySetVersion::Version00),
            Err(Error::InvalidPublicKeySize {
                expected: 33,
                found: 48
            })
        ));
    }
}

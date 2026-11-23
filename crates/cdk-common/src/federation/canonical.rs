//! Canonical byte encodings that federation identifiers and signatures hash.
//!
//! Every member must produce identical bytes for the same value, so each
//! encoding has exactly one definition here.

use serde::Serialize;

use super::Result;

/// Serializes `value` as JSON with every object's keys sorted.
///
/// Field order in the Rust type and in received JSON does not change the
/// bytes, so hashes over the result are stable across members.
pub fn canonical_json_bytes<T>(value: &T) -> Result<Vec<u8>>
where
    T: Serialize + ?Sized,
{
    let mut value = serde_json::to_value(value)?;
    value.sort_all_objects();
    Ok(serde_json::to_vec(&value)?)
}

/// Appends `value` to `transcript` behind a big-endian `u32` length prefix.
///
/// The prefix makes concatenated fields unambiguous inside a hash transcript.
pub fn append_hash_bytes(transcript: &mut Vec<u8>, value: &[u8]) {
    transcript.extend_from_slice(&(value.len() as u32).to_be_bytes());
    transcript.extend_from_slice(value);
}

/// Implements byte access and lowercase-hex `Display`, `FromStr` and serde for
/// a `struct $name([u8; $len])` newtype.
///
/// `$size_error` names the [`FederationError`](super::FederationError)
/// variant, with `expected` and `found` fields, returned for input of the
/// wrong length.
macro_rules! impl_fixed_bytes {
    ($name:ident, $len:literal, $size_error:ident) => {
        impl $name {
            /// Construct from raw bytes.
            #[must_use]
            pub fn from_bytes(bytes: [u8; $len]) -> Self {
                Self(bytes)
            }

            /// Parse from a byte slice of exactly the right length.
            pub fn from_slice(bytes: &[u8]) -> $crate::federation::Result<Self> {
                let bytes: [u8; $len] = bytes.try_into().map_err(|_| {
                    $crate::federation::FederationError::$size_error {
                        expected: $len,
                        found: bytes.len(),
                    }
                })?;
                Ok(Self(bytes))
            }

            /// Parse from hexadecimal.
            pub fn from_hex<S>(hex: S) -> $crate::federation::Result<Self>
            where
                S: AsRef<str>,
            {
                let bytes = $crate::util::hex::decode(hex.as_ref())?;
                Self::from_slice(&bytes)
            }

            /// Return the raw bytes.
            #[must_use]
            pub fn to_bytes(self) -> [u8; $len] {
                self.0
            }

            /// Return lowercase hexadecimal.
            #[must_use]
            pub fn to_hex(self) -> String {
                $crate::util::hex::encode(self.0)
            }
        }

        impl core::fmt::Display for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str(&self.to_hex())
            }
        }

        impl core::str::FromStr for $name {
            type Err = $crate::federation::FederationError;

            fn from_str(value: &str) -> $crate::federation::Result<Self> {
                Self::from_hex(value)
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serializer.serialize_str(&self.to_hex())
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = <String as serde::Deserialize>::deserialize(deserializer)?;
                Self::from_hex(value).map_err(serde::de::Error::custom)
            }
        }
    };
}
pub(crate) use impl_fixed_bytes;

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn canonical_json_sorts_nested_object_keys() {
        let first = json!({"b": 1, "a": {"d": [{"z": 1, "y": 2}], "c": null}});
        let second = json!({"a": {"c": null, "d": [{"y": 2, "z": 1}]}, "b": 1});

        let bytes = canonical_json_bytes(&first).expect("encode first");
        assert_eq!(bytes, canonical_json_bytes(&second).expect("encode second"));
        assert_eq!(
            bytes,
            br#"{"a":{"c":null,"d":[{"y":2,"z":1}]},"b":1}"#.to_vec()
        );
    }

    #[test]
    fn hash_bytes_are_length_prefixed() {
        let mut transcript = Vec::new();
        append_hash_bytes(&mut transcript, b"ab");
        append_hash_bytes(&mut transcript, b"");

        assert_eq!(transcript, vec![0, 0, 0, 2, b'a', b'b', 0, 0, 0, 0]);
    }
}

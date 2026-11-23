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

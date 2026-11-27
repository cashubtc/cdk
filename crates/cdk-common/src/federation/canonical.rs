//! Canonical byte encodings that federation identifiers and signatures hash.
//!
//! Every member must produce identical bytes for the same value, so each
//! encoding has exactly one definition here.

use std::borrow::Cow;
use std::ops::Range;

use serde::ser::{
    self, Error as _, SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant,
    SerializeTuple, SerializeTupleStruct, SerializeTupleVariant, Serializer,
};
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
    let mut out = Vec::with_capacity(256);
    value.serialize(CanonicalJsonSerializer { out: &mut out })?;
    Ok(out)
}

struct CanonicalJsonSerializer<'a> {
    out: &'a mut Vec<u8>,
}

struct CanonicalSeqSerializer<'a> {
    out: &'a mut Vec<u8>,
    first: bool,
    closing_brace: bool,
}

struct CanonicalObjectSerializer<'a> {
    out: &'a mut Vec<u8>,
    values: Vec<u8>,
    entries: Vec<(Cow<'static, str>, Range<usize>)>,
    pending_key: Option<Cow<'static, str>>,
    sorted: bool,
    closing_brace: bool,
}

impl<'a> CanonicalObjectSerializer<'a> {
    fn new(out: &'a mut Vec<u8>, len: Option<usize>, closing_brace: bool) -> Self {
        Self {
            out,
            values: Vec::with_capacity(len.unwrap_or(4).saturating_mul(32)),
            entries: Vec::with_capacity(len.unwrap_or(4)),
            pending_key: None,
            sorted: true,
            closing_brace,
        }
    }

    fn push_entry<T>(&mut self, key: Cow<'static, str>, value: &T) -> serde_json::Result<()>
    where
        T: Serialize + ?Sized,
    {
        if self.sorted {
            if let Some((last_key, _)) = self.entries.last() {
                if last_key.as_ref() >= key.as_ref() {
                    self.sorted = false;
                }
            }
        }
        let start = self.values.len();
        value.serialize(CanonicalJsonSerializer {
            out: &mut self.values,
        })?;
        let end = self.values.len();
        self.entries.push((key, start..end));
        Ok(())
    }

    fn finish(mut self) -> serde_json::Result<()> {
        if !self.sorted {
            self.entries.sort_by(|left, right| left.0.cmp(&right.0));
        }
        self.out.push(b'{');
        let mut wrote_any = false;
        let len = self.entries.len();
        for index in 0..len {
            // Preserve `serde_json::Map` last-value-wins semantics for duplicate keys.
            if index + 1 < len && self.entries[index].0 == self.entries[index + 1].0 {
                continue;
            }
            if wrote_any {
                self.out.push(b',');
            }
            wrote_any = true;
            let (ref key, ref range) = self.entries[index];
            serde_json::to_writer(&mut *self.out, key.as_ref())?;
            self.out.push(b':');
            self.out.extend_from_slice(&self.values[range.clone()]);
        }
        self.out.push(b'}');
        if self.closing_brace {
            self.out.push(b'}');
        }
        Ok(())
    }
}

impl<'a> Serializer for CanonicalJsonSerializer<'a> {
    type Ok = ();
    type Error = serde_json::Error;

    type SerializeSeq = CanonicalSeqSerializer<'a>;
    type SerializeTuple = CanonicalSeqSerializer<'a>;
    type SerializeTupleStruct = CanonicalSeqSerializer<'a>;
    type SerializeTupleVariant = CanonicalSeqSerializer<'a>;
    type SerializeMap = CanonicalObjectSerializer<'a>;
    type SerializeStruct = CanonicalObjectSerializer<'a>;
    type SerializeStructVariant = CanonicalObjectSerializer<'a>;

    fn serialize_bool(self, v: bool) -> serde_json::Result<()> {
        self.out
            .extend_from_slice(if v { b"true" } else { b"false" });
        Ok(())
    }

    fn serialize_i8(self, v: i8) -> serde_json::Result<()> {
        serde_json::to_writer(&mut *self.out, &v)
    }

    fn serialize_i16(self, v: i16) -> serde_json::Result<()> {
        serde_json::to_writer(&mut *self.out, &v)
    }

    fn serialize_i32(self, v: i32) -> serde_json::Result<()> {
        serde_json::to_writer(&mut *self.out, &v)
    }

    fn serialize_i64(self, v: i64) -> serde_json::Result<()> {
        serde_json::to_writer(&mut *self.out, &v)
    }

    fn serialize_i128(self, v: i128) -> serde_json::Result<()> {
        serde_json::to_writer(&mut *self.out, &v)
    }

    fn serialize_u8(self, v: u8) -> serde_json::Result<()> {
        serde_json::to_writer(&mut *self.out, &v)
    }

    fn serialize_u16(self, v: u16) -> serde_json::Result<()> {
        serde_json::to_writer(&mut *self.out, &v)
    }

    fn serialize_u32(self, v: u32) -> serde_json::Result<()> {
        serde_json::to_writer(&mut *self.out, &v)
    }

    fn serialize_u64(self, v: u64) -> serde_json::Result<()> {
        serde_json::to_writer(&mut *self.out, &v)
    }

    fn serialize_u128(self, v: u128) -> serde_json::Result<()> {
        serde_json::to_writer(&mut *self.out, &v)
    }

    fn serialize_f32(self, v: f32) -> serde_json::Result<()> {
        serde_json::to_writer(&mut *self.out, &v)
    }

    fn serialize_f64(self, v: f64) -> serde_json::Result<()> {
        serde_json::to_writer(&mut *self.out, &v)
    }

    fn serialize_char(self, v: char) -> serde_json::Result<()> {
        serde_json::to_writer(&mut *self.out, &v)
    }

    fn serialize_str(self, v: &str) -> serde_json::Result<()> {
        serde_json::to_writer(&mut *self.out, v)
    }

    fn serialize_bytes(self, v: &[u8]) -> serde_json::Result<()> {
        self.out.push(b'[');
        for (index, byte) in v.iter().enumerate() {
            if index > 0 {
                self.out.push(b',');
            }
            serde_json::to_writer(&mut *self.out, byte)?;
        }
        self.out.push(b']');
        Ok(())
    }

    fn serialize_none(self) -> serde_json::Result<()> {
        self.out.extend_from_slice(b"null");
        Ok(())
    }

    fn serialize_some<T>(self, value: &T) -> serde_json::Result<()>
    where
        T: Serialize + ?Sized,
    {
        value.serialize(self)
    }

    fn serialize_unit(self) -> serde_json::Result<()> {
        self.out.extend_from_slice(b"null");
        Ok(())
    }

    fn serialize_unit_struct(self, _name: &'static str) -> serde_json::Result<()> {
        self.out.extend_from_slice(b"null");
        Ok(())
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> serde_json::Result<()> {
        serde_json::to_writer(&mut *self.out, variant)
    }

    fn serialize_newtype_struct<T>(self, _name: &'static str, value: &T) -> serde_json::Result<()>
    where
        T: Serialize + ?Sized,
    {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T>(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> serde_json::Result<()>
    where
        T: Serialize + ?Sized,
    {
        self.out.push(b'{');
        serde_json::to_writer(&mut *self.out, variant)?;
        self.out.push(b':');
        value.serialize(CanonicalJsonSerializer { out: self.out })?;
        self.out.push(b'}');
        Ok(())
    }

    fn serialize_seq(self, _len: Option<usize>) -> serde_json::Result<Self::SerializeSeq> {
        self.out.push(b'[');
        Ok(CanonicalSeqSerializer {
            out: self.out,
            first: true,
            closing_brace: false,
        })
    }

    fn serialize_tuple(self, len: usize) -> serde_json::Result<Self::SerializeTuple> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> serde_json::Result<Self::SerializeTupleStruct> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        _len: usize,
    ) -> serde_json::Result<Self::SerializeTupleVariant> {
        self.out.push(b'{');
        serde_json::to_writer(&mut *self.out, variant)?;
        self.out.extend_from_slice(b":[");
        Ok(CanonicalSeqSerializer {
            out: self.out,
            first: true,
            closing_brace: true,
        })
    }

    fn serialize_map(self, len: Option<usize>) -> serde_json::Result<Self::SerializeMap> {
        Ok(CanonicalObjectSerializer::new(self.out, len, false))
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> serde_json::Result<Self::SerializeStruct> {
        Ok(CanonicalObjectSerializer::new(self.out, Some(len), false))
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> serde_json::Result<Self::SerializeStructVariant> {
        self.out.push(b'{');
        serde_json::to_writer(&mut *self.out, variant)?;
        self.out.push(b':');
        Ok(CanonicalObjectSerializer::new(self.out, Some(len), true))
    }
}

impl SerializeSeq for CanonicalSeqSerializer<'_> {
    type Ok = ();
    type Error = serde_json::Error;

    fn serialize_element<T>(&mut self, value: &T) -> serde_json::Result<()>
    where
        T: Serialize + ?Sized,
    {
        if !self.first {
            self.out.push(b',');
        }
        self.first = false;
        value.serialize(CanonicalJsonSerializer { out: self.out })
    }

    fn end(self) -> serde_json::Result<()> {
        self.out.push(b']');
        if self.closing_brace {
            self.out.push(b'}');
        }
        Ok(())
    }
}

impl SerializeTuple for CanonicalSeqSerializer<'_> {
    type Ok = ();
    type Error = serde_json::Error;

    fn serialize_element<T>(&mut self, value: &T) -> serde_json::Result<()>
    where
        T: Serialize + ?Sized,
    {
        SerializeSeq::serialize_element(self, value)
    }

    fn end(self) -> serde_json::Result<()> {
        SerializeSeq::end(self)
    }
}

impl SerializeTupleStruct for CanonicalSeqSerializer<'_> {
    type Ok = ();
    type Error = serde_json::Error;

    fn serialize_field<T>(&mut self, value: &T) -> serde_json::Result<()>
    where
        T: Serialize + ?Sized,
    {
        SerializeSeq::serialize_element(self, value)
    }

    fn end(self) -> serde_json::Result<()> {
        SerializeSeq::end(self)
    }
}

impl SerializeTupleVariant for CanonicalSeqSerializer<'_> {
    type Ok = ();
    type Error = serde_json::Error;

    fn serialize_field<T>(&mut self, value: &T) -> serde_json::Result<()>
    where
        T: Serialize + ?Sized,
    {
        SerializeSeq::serialize_element(self, value)
    }

    fn end(self) -> serde_json::Result<()> {
        SerializeSeq::end(self)
    }
}

impl SerializeMap for CanonicalObjectSerializer<'_> {
    type Ok = ();
    type Error = serde_json::Error;

    fn serialize_key<T>(&mut self, key: &T) -> serde_json::Result<()>
    where
        T: Serialize + ?Sized,
    {
        self.pending_key = Some(key.serialize(CanonicalMapKeySerializer)?);
        Ok(())
    }

    fn serialize_value<T>(&mut self, value: &T) -> serde_json::Result<()>
    where
        T: Serialize + ?Sized,
    {
        let key = self.pending_key.take().ok_or_else(|| {
            serde_json::Error::custom("serialize_value called before serialize_key")
        })?;
        self.push_entry(key, value)
    }

    fn serialize_entry<K, V>(&mut self, key: &K, value: &V) -> serde_json::Result<()>
    where
        K: Serialize + ?Sized,
        V: Serialize + ?Sized,
    {
        let key = key.serialize(CanonicalMapKeySerializer)?;
        self.push_entry(key, value)
    }

    fn end(self) -> serde_json::Result<()> {
        self.finish()
    }
}

impl SerializeStruct for CanonicalObjectSerializer<'_> {
    type Ok = ();
    type Error = serde_json::Error;

    fn serialize_field<T>(&mut self, key: &'static str, value: &T) -> serde_json::Result<()>
    where
        T: Serialize + ?Sized,
    {
        self.push_entry(Cow::Borrowed(key), value)
    }

    fn end(self) -> serde_json::Result<()> {
        self.finish()
    }
}

impl SerializeStructVariant for CanonicalObjectSerializer<'_> {
    type Ok = ();
    type Error = serde_json::Error;

    fn serialize_field<T>(&mut self, key: &'static str, value: &T) -> serde_json::Result<()>
    where
        T: Serialize + ?Sized,
    {
        self.push_entry(Cow::Borrowed(key), value)
    }

    fn end(self) -> serde_json::Result<()> {
        self.finish()
    }
}

struct CanonicalMapKeySerializer;

fn key_must_be_string() -> serde_json::Error {
    serde_json::Error::custom("key must be a string")
}

impl Serializer for CanonicalMapKeySerializer {
    type Ok = Cow<'static, str>;
    type Error = serde_json::Error;

    type SerializeSeq = ser::Impossible<Cow<'static, str>, serde_json::Error>;
    type SerializeTuple = ser::Impossible<Cow<'static, str>, serde_json::Error>;
    type SerializeTupleStruct = ser::Impossible<Cow<'static, str>, serde_json::Error>;
    type SerializeTupleVariant = ser::Impossible<Cow<'static, str>, serde_json::Error>;
    type SerializeMap = ser::Impossible<Cow<'static, str>, serde_json::Error>;
    type SerializeStruct = ser::Impossible<Cow<'static, str>, serde_json::Error>;
    type SerializeStructVariant = ser::Impossible<Cow<'static, str>, serde_json::Error>;

    fn serialize_bool(self, v: bool) -> serde_json::Result<Self::Ok> {
        Ok(Cow::Borrowed(if v { "true" } else { "false" }))
    }

    fn serialize_i8(self, v: i8) -> serde_json::Result<Self::Ok> {
        Ok(Cow::Owned(v.to_string()))
    }

    fn serialize_i16(self, v: i16) -> serde_json::Result<Self::Ok> {
        Ok(Cow::Owned(v.to_string()))
    }

    fn serialize_i32(self, v: i32) -> serde_json::Result<Self::Ok> {
        Ok(Cow::Owned(v.to_string()))
    }

    fn serialize_i64(self, v: i64) -> serde_json::Result<Self::Ok> {
        Ok(Cow::Owned(v.to_string()))
    }

    fn serialize_i128(self, v: i128) -> serde_json::Result<Self::Ok> {
        Ok(Cow::Owned(v.to_string()))
    }

    fn serialize_u8(self, v: u8) -> serde_json::Result<Self::Ok> {
        Ok(Cow::Owned(v.to_string()))
    }

    fn serialize_u16(self, v: u16) -> serde_json::Result<Self::Ok> {
        Ok(Cow::Owned(v.to_string()))
    }

    fn serialize_u32(self, v: u32) -> serde_json::Result<Self::Ok> {
        Ok(Cow::Owned(v.to_string()))
    }

    fn serialize_u64(self, v: u64) -> serde_json::Result<Self::Ok> {
        Ok(Cow::Owned(v.to_string()))
    }

    fn serialize_u128(self, v: u128) -> serde_json::Result<Self::Ok> {
        Ok(Cow::Owned(v.to_string()))
    }

    fn serialize_f32(self, _v: f32) -> serde_json::Result<Self::Ok> {
        Err(key_must_be_string())
    }

    fn serialize_f64(self, _v: f64) -> serde_json::Result<Self::Ok> {
        Err(key_must_be_string())
    }

    fn serialize_char(self, v: char) -> serde_json::Result<Self::Ok> {
        Ok(Cow::Owned(v.to_string()))
    }

    fn serialize_str(self, v: &str) -> serde_json::Result<Self::Ok> {
        Ok(Cow::Owned(v.to_owned()))
    }

    fn serialize_bytes(self, _v: &[u8]) -> serde_json::Result<Self::Ok> {
        Err(key_must_be_string())
    }

    fn serialize_none(self) -> serde_json::Result<Self::Ok> {
        Err(key_must_be_string())
    }

    fn serialize_some<T>(self, _value: &T) -> serde_json::Result<Self::Ok>
    where
        T: Serialize + ?Sized,
    {
        Err(key_must_be_string())
    }

    fn serialize_unit(self) -> serde_json::Result<Self::Ok> {
        Err(key_must_be_string())
    }

    fn serialize_unit_struct(self, _name: &'static str) -> serde_json::Result<Self::Ok> {
        Err(key_must_be_string())
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> serde_json::Result<Self::Ok> {
        Ok(Cow::Borrowed(variant))
    }

    fn serialize_newtype_struct<T>(
        self,
        _name: &'static str,
        value: &T,
    ) -> serde_json::Result<Self::Ok>
    where
        T: Serialize + ?Sized,
    {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T>(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _value: &T,
    ) -> serde_json::Result<Self::Ok>
    where
        T: Serialize + ?Sized,
    {
        Err(key_must_be_string())
    }

    fn serialize_seq(self, _len: Option<usize>) -> serde_json::Result<Self::SerializeSeq> {
        Err(key_must_be_string())
    }

    fn serialize_tuple(self, _len: usize) -> serde_json::Result<Self::SerializeTuple> {
        Err(key_must_be_string())
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> serde_json::Result<Self::SerializeTupleStruct> {
        Err(key_must_be_string())
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> serde_json::Result<Self::SerializeTupleVariant> {
        Err(key_must_be_string())
    }

    fn serialize_map(self, _len: Option<usize>) -> serde_json::Result<Self::SerializeMap> {
        Err(key_must_be_string())
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> serde_json::Result<Self::SerializeStruct> {
        Err(key_must_be_string())
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> serde_json::Result<Self::SerializeStructVariant> {
        Err(key_must_be_string())
    }
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
/// `$kind` names the value in the
/// [`FederationError::InvalidByteLength`](super::FederationError::InvalidByteLength)
/// returned for input of the wrong length.
macro_rules! impl_fixed_bytes {
    ($name:ident, $len:literal, $kind:literal) => {
        impl $name {
            /// Construct from raw bytes.
            #[must_use]
            pub fn from_bytes(bytes: [u8; $len]) -> Self {
                Self(bytes)
            }

            /// Parse from a byte slice of exactly the right length.
            pub fn from_slice(bytes: &[u8]) -> $crate::federation::Result<Self> {
                let bytes: [u8; $len] = bytes.try_into().map_err(|_| {
                    $crate::federation::FederationError::InvalidByteLength {
                        kind: $kind,
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
    use std::collections::BTreeMap;

    use serde::ser::SerializeMap;
    use serde::{Serialize, Serializer};
    use serde_json::json;

    use super::*;

    fn reference_canonical_json_bytes<T>(value: &T) -> Vec<u8>
    where
        T: Serialize + ?Sized,
    {
        let mut value = serde_json::to_value(value).expect("to_value");
        value.sort_all_objects();
        serde_json::to_vec(&value).expect("to_vec")
    }

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
        assert_eq!(bytes, reference_canonical_json_bytes(&first));
    }

    #[test]
    fn canonical_json_matches_reference_across_rust_types_and_edge_cases() {
        #[derive(Serialize)]
        enum SampleEnum {
            Unit,
            Newtype(u64),
            Tuple(u8, &'static str),
            Struct { z_field: bool, a_field: i64 },
        }

        #[derive(Serialize)]
        struct SampleStruct {
            z_str: &'static str,
            m_map: BTreeMap<u16, SampleEnum>,
            a_opt: Option<u32>,
            b_none: Option<u32>,
            c_float: f64,
            d_nan: f64,
            e_tuple: (i32, Vec<u8>),
        }

        let mut map = BTreeMap::new();
        map.insert(10, SampleEnum::Unit);
        map.insert(2, SampleEnum::Newtype(42));
        map.insert(3, SampleEnum::Tuple(7, "quote\"and\\slash\n\u{0001}"));
        map.insert(
            1,
            SampleEnum::Struct {
                z_field: true,
                a_field: -99,
            },
        );

        let sample = SampleStruct {
            z_str: "hello\tworld\"\\",
            m_map: map,
            a_opt: Some(123),
            b_none: None,
            c_float: 1.5,
            d_nan: f64::NAN,
            e_tuple: (-5, vec![0, 1, 255]),
        };

        let bytes = canonical_json_bytes(&sample).expect("canonical sample");
        assert_eq!(bytes, reference_canonical_json_bytes(&sample));
    }

    #[test]
    fn canonical_json_deduplicates_map_keys_keeping_last_value() {
        struct DuplicateKeyMap;

        impl Serialize for DuplicateKeyMap {
            fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                let mut map = serializer.serialize_map(Some(3))?;
                map.serialize_entry("b", &1)?;
                map.serialize_entry("a", &2)?;
                map.serialize_entry("b", &3)?;
                map.end()
            }
        }

        let bytes = canonical_json_bytes(&DuplicateKeyMap).expect("canonical duplicate map");
        assert_eq!(bytes, br#"{"a":2,"b":3}"#.to_vec());
        assert_eq!(bytes, reference_canonical_json_bytes(&DuplicateKeyMap));
    }

    #[test]
    fn canonical_json_matches_serde_json_for_bool_map_keys() {
        let mut map = BTreeMap::new();
        map.insert(true, 1_u8);
        map.insert(false, 0_u8);
        let bytes = canonical_json_bytes(&map).expect("canonical bool map");
        assert_eq!(bytes, br#"{"false":0,"true":1}"#.to_vec());
        assert_eq!(bytes, reference_canonical_json_bytes(&map));
    }

    #[test]
    fn hash_bytes_are_length_prefixed() {
        let mut transcript = Vec::new();
        append_hash_bytes(&mut transcript, b"ab");
        append_hash_bytes(&mut transcript, b"");

        assert_eq!(transcript, vec![0, 0, 0, 2, b'a', b'b', 0, 0, 0, 0]);
    }
}

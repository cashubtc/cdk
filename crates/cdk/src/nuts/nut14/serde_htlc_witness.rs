//! Serde helpers for HTLC Witness

use serde::{de, ser, Deserialize, Deserializer, Serializer};

use super::HTLCWitness;

/// Serialize [HTLCWitness] as stringified JSON
pub fn serialize<S>(x: &HTLCWitness, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    s.serialize_str(&serde_json::to_string(&x).map_err(ser::Error::custom)?)
}

/// Deserialize [HTLCWitness] from stringified JSON
pub fn deserialize<'de, D>(deserializer: D) -> Result<HTLCWitness, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = String::deserialize(deserializer)?;
    serde_json::from_str(&s).map_err(de::Error::custom)
}

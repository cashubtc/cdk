//! Serde utils for P2PK Witness

use serde::{de, ser, Deserialize, Deserializer, Serializer};

use super::SCTWitness;

/// Serialize [sctWitness] as stringified JSON
pub fn serialize<S>(x: &SCTWitness, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    s.serialize_str(&serde_json::to_string(&x).map_err(ser::Error::custom)?)
}

/// Deserialize [sctWitness] from stringified JSON
pub fn deserialize<'de, D>(deserializer: D) -> Result<SCTWitness, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = String::deserialize(deserializer)?;
    serde_json::from_str(&s).map_err(de::Error::custom)
}

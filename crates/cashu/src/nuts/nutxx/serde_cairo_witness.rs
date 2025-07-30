//! Serde utils for Cairo Witness

use serde::{de, ser, Deserialize, Deserializer, Serializer};

use super::CairoWitness;

/// Serialize [CairoWitness] as stringified JSON
pub fn serialize<S>(x: &CairoWitness, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    s.serialize_str(&serde_json::to_string(&x).map_err(ser::Error::custom)?)
}

/// Deserialize [CairoWitness] from stringified JSON
pub fn deserialize<'de, D>(deserializer: D) -> Result<CairoWitness, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = String::deserialize(deserializer)?;
    serde_json::from_str(&s).map_err(de::Error::custom)
}

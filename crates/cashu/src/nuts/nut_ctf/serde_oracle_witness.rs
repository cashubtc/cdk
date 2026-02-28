//! Serde utils for Oracle Witness (NUT-CTF)

use serde::{de, ser, Deserialize, Deserializer, Serializer};

use super::OracleWitness;

/// Serialize [OracleWitness] as stringified JSON
pub(crate) fn serialize<S>(x: &OracleWitness, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    s.serialize_str(&serde_json::to_string(&x).map_err(ser::Error::custom)?)
}

/// Deserialize [OracleWitness] from stringified JSON
pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<OracleWitness, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = String::deserialize(deserializer)?;
    serde_json::from_str(&s).map_err(de::Error::custom)
}

//! Serde helper for Bolt12 `Offer` fields, serialized as their string form.

use std::str::FromStr;

use serde::{self, Deserialize, Deserializer, Serializer};

use super::Offer;

pub fn serialize<S>(offer: &Offer, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let s = offer.to_string();
    serializer.serialize_str(&s)
}

pub fn deserialize<'de, D>(deserializer: D) -> Result<Box<Offer>, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    Ok(Box::new(Offer::from_str(&s).map_err(|_| {
        serde::de::Error::custom("Invalid Bolt12 Offer")
    })?))
}

//! Serde helper for `Amount<CurrencyUnit>` fields.
//!
//! `Amount<U>` serializes as a bare `u64` and only `Amount<()>` implements
//! `Deserialize` (the wire form drops the unit). This module serializes the
//! value together with its unit so a typed amount round-trips inside a derived
//! `Serialize`/`Deserialize` struct.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{Amount, CurrencyUnit};

#[derive(Serialize, Deserialize)]
struct Repr {
    value: u64,
    unit: CurrencyUnit,
}

pub fn serialize<S>(amount: &Amount<CurrencyUnit>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    Repr {
        value: amount.value(),
        unit: amount.unit().clone(),
    }
    .serialize(serializer)
}

pub fn deserialize<'de, D>(deserializer: D) -> Result<Amount<CurrencyUnit>, D::Error>
where
    D: Deserializer<'de>,
{
    let repr = Repr::deserialize(deserializer)?;
    Ok(Amount::new(repr.value, repr.unit))
}

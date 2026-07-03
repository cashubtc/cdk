//! Serde helper for `Option<Amount<CurrencyUnit>>` fields.
//!
//! Mirrors the `amount_currency_serde` module but keeps the outer `Option` so
//! an absent amount round-trips as `null`.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{Amount, CurrencyUnit};

#[derive(Serialize, Deserialize)]
struct Repr {
    value: u64,
    unit: CurrencyUnit,
}

pub fn serialize<S>(amount: &Option<Amount<CurrencyUnit>>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    amount
        .as_ref()
        .map(|a| Repr {
            value: a.value(),
            unit: a.unit().clone(),
        })
        .serialize(serializer)
}

pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Amount<CurrencyUnit>>, D::Error>
where
    D: Deserializer<'de>,
{
    let repr = Option::<Repr>::deserialize(deserializer)?;
    Ok(repr.map(|r| Amount::new(r.value, r.unit)))
}

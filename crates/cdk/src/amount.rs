//! CDK Amount
//!
//! Is any unit and will be treated as the unit of the wallet

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use crate::nuts::CurrencyUnit;

/// Amount Error
#[derive(Debug, Error)]
pub enum Error {
    /// Split Values must be less then or equal to amount
    #[error("Split Values must be less then or equal to amount")]
    SplitValuesGreater,
    /// Amount overflow
    #[error("Amount Overflow")]
    AmountOverflow,
    /// Cannot convert units
    #[error("Cannot convert units")]
    CannotConvertUnits,
}

/// Amount can be any unit
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Amount(u64);

impl Amount {
    /// Amount zero
    pub const ZERO: Amount = Amount(0);

    /// Split into parts that are powers of two
    pub fn split(&self) -> Vec<Self> {
        let sats = self.0;
        (0_u64..64)
            .rev()
            .filter_map(|bit| {
                let part = 1 << bit;
                ((sats & part) == part).then_some(Self::from(part))
            })
            .collect()
    }

    /// Split into parts that are powers of two by target
    pub fn split_targeted(&self, target: &SplitTarget) -> Result<Vec<Self>, Error> {
        let mut parts = match target {
            SplitTarget::None => self.split(),
            SplitTarget::Value(amount) => {
                if self.le(amount) {
                    return Ok(self.split());
                }

                let mut parts_total = Amount::ZERO;
                let mut parts = Vec::new();

                // The powers of two that are need to create target value
                let parts_of_value = amount.split();

                while parts_total.lt(self) {
                    for part in parts_of_value.iter().copied() {
                        if (part + parts_total).le(self) {
                            parts.push(part);
                        } else {
                            let amount_left = *self - parts_total;
                            parts.extend(amount_left.split());
                        }

                        parts_total = Amount::try_sum(parts.clone().iter().copied())?;

                        if parts_total.eq(self) {
                            break;
                        }
                    }
                }

                parts
            }
            SplitTarget::Values(values) => {
                let values_total: Amount = Amount::try_sum(values.clone().into_iter())?;

                match self.cmp(&values_total) {
                    Ordering::Equal => values.clone(),
                    Ordering::Less => {
                        return Err(Error::SplitValuesGreater);
                    }
                    Ordering::Greater => {
                        let extra = *self - values_total;
                        let mut extra_amount = extra.split();
                        let mut values = values.clone();

                        values.append(&mut extra_amount);
                        values
                    }
                }
            }
        };

        parts.sort();
        Ok(parts)
    }

    /// Checked addition for Amount. Returns None if overflow occurs.
    pub fn checked_add(self, other: Amount) -> Option<Amount> {
        self.0.checked_add(other.0).map(Amount)
    }

    /// Checked subtraction for Amount. Returns None if overflow occurs.
    pub fn checked_sub(self, other: Amount) -> Option<Amount> {
        self.0.checked_sub(other.0).map(Amount)
    }

    /// Try sum to check for overflow
    pub fn try_sum<I>(iter: I) -> Result<Self, Error>
    where
        I: IntoIterator<Item = Self>,
    {
        iter.into_iter().try_fold(Amount::ZERO, |acc, x| {
            acc.checked_add(x).ok_or(Error::AmountOverflow)
        })
    }
}

impl Default for Amount {
    fn default() -> Self {
        Amount::ZERO
    }
}

impl Default for &Amount {
    fn default() -> Self {
        &Amount::ZERO
    }
}

impl fmt::Display for Amount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(width) = f.width() {
            write!(f, "{:width$}", self.0, width = width)
        } else {
            write!(f, "{}", self.0)
        }
    }
}

impl From<u64> for Amount {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl From<&u64> for Amount {
    fn from(value: &u64) -> Self {
        Self(*value)
    }
}

impl From<Amount> for u64 {
    fn from(value: Amount) -> Self {
        value.0
    }
}

impl AsRef<u64> for Amount {
    fn as_ref(&self) -> &u64 {
        &self.0
    }
}

impl std::ops::Add for Amount {
    type Output = Amount;

    fn add(self, rhs: Amount) -> Self::Output {
        Amount(self.0.checked_add(rhs.0).expect("Addition error"))
    }
}

impl std::ops::AddAssign for Amount {
    fn add_assign(&mut self, rhs: Self) {
        self.0 = self.0.checked_add(rhs.0).expect("Addition error");
    }
}

impl std::ops::Sub for Amount {
    type Output = Amount;

    fn sub(self, rhs: Amount) -> Self::Output {
        Amount(self.0 - rhs.0)
    }
}

impl std::ops::SubAssign for Amount {
    fn sub_assign(&mut self, other: Self) {
        self.0 -= other.0;
    }
}

impl std::ops::Mul for Amount {
    type Output = Self;

    fn mul(self, other: Self) -> Self::Output {
        Amount(self.0 * other.0)
    }
}

impl std::ops::Div for Amount {
    type Output = Self;

    fn div(self, other: Self) -> Self::Output {
        Amount(self.0 / other.0)
    }
}

/// String wrapper for an [Amount].
///
/// It ser-/deserializes the inner [Amount] to a string, while at the same time using the [u64]
/// value of the [Amount] for comparison and ordering. This helps automatically sort the keys of
/// a [BTreeMap] when [AmountStr] is used as key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmountStr(Amount);

impl AmountStr {
    pub(crate) fn from(amt: Amount) -> Self {
        Self(amt)
    }
}

impl PartialOrd<Self> for AmountStr {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for AmountStr {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.cmp(&other.0)
    }
}

impl<'de> Deserialize<'de> for AmountStr {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        u64::from_str(&s)
            .map(Amount)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

impl Serialize for AmountStr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

/// Kinds of targeting that are supported
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub enum SplitTarget {
    /// Default target; least amount of proofs
    #[default]
    None,
    /// Target amount for wallet to have most proofs that add up to value
    Value(Amount),
    /// Specific amounts to split into **MUST** equal amount being split
    Values(Vec<Amount>),
}

/// Msats in sat
pub const MSAT_IN_SAT: u64 = 1000;

/// Helper function to convert units
pub fn to_unit<T>(
    amount: T,
    current_unit: &CurrencyUnit,
    target_unit: &CurrencyUnit,
) -> Result<Amount, Error>
where
    T: Into<u64>,
{
    let amount = amount.into();
    match (current_unit, target_unit) {
        (CurrencyUnit::Sat, CurrencyUnit::Sat) => Ok(amount.into()),
        (CurrencyUnit::Msat, CurrencyUnit::Msat) => Ok(amount.into()),
        (CurrencyUnit::Sat, CurrencyUnit::Msat) => Ok((amount * MSAT_IN_SAT).into()),
        (CurrencyUnit::Msat, CurrencyUnit::Sat) => Ok((amount / MSAT_IN_SAT).into()),
        (CurrencyUnit::Usd, CurrencyUnit::Usd) => Ok(amount.into()),
        (CurrencyUnit::Eur, CurrencyUnit::Eur) => Ok(amount.into()),
        _ => Err(Error::CannotConvertUnits),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_amount() {
        assert_eq!(Amount::from(1).split(), vec![Amount::from(1)]);
        assert_eq!(Amount::from(2).split(), vec![Amount::from(2)]);
        assert_eq!(
            Amount::from(3).split(),
            vec![Amount::from(2), Amount::from(1)]
        );
        let amounts: Vec<Amount> = [8, 2, 1].iter().map(|a| Amount::from(*a)).collect();
        assert_eq!(Amount::from(11).split(), amounts);
        let amounts: Vec<Amount> = [128, 64, 32, 16, 8, 4, 2, 1]
            .iter()
            .map(|a| Amount::from(*a))
            .collect();
        assert_eq!(Amount::from(255).split(), amounts);
    }

    #[test]
    fn test_split_target_amount() {
        let amount = Amount(65);

        let split = amount
            .split_targeted(&SplitTarget::Value(Amount(32)))
            .unwrap();
        assert_eq!(vec![Amount(1), Amount(32), Amount(32)], split);

        let amount = Amount(150);

        let split = amount
            .split_targeted(&SplitTarget::Value(Amount::from(50)))
            .unwrap();
        assert_eq!(
            vec![
                Amount(2),
                Amount(2),
                Amount(2),
                Amount(16),
                Amount(16),
                Amount(16),
                Amount(32),
                Amount(32),
                Amount(32)
            ],
            split
        );

        let amount = Amount::from(63);

        let split = amount
            .split_targeted(&SplitTarget::Value(Amount::from(32)))
            .unwrap();
        assert_eq!(
            vec![
                Amount(1),
                Amount(2),
                Amount(4),
                Amount(8),
                Amount(16),
                Amount(32)
            ],
            split
        );
    }

    #[test]
    fn test_split_values() {
        let amount = Amount(10);

        let target = vec![Amount(2), Amount(4), Amount(4)];

        let split_target = SplitTarget::Values(target.clone());

        let values = amount.split_targeted(&split_target).unwrap();

        assert_eq!(target, values);

        let target = vec![Amount(2), Amount(4), Amount(4)];

        let split_target = SplitTarget::Values(vec![Amount(2), Amount(4)]);

        let values = amount.split_targeted(&split_target).unwrap();

        assert_eq!(target, values);

        let split_target = SplitTarget::Values(vec![Amount(2), Amount(10)]);

        let values = amount.split_targeted(&split_target);

        assert!(values.is_err())
    }

    #[test]
    #[should_panic]
    fn test_amount_addition() {
        let amount_one: Amount = u64::MAX.into();
        let amount_two: Amount = 1.into();

        let amounts = vec![amount_one, amount_two];

        let _total: Amount = Amount::try_sum(amounts).unwrap();
    }

    #[test]
    fn test_try_amount_addition() {
        let amount_one: Amount = u64::MAX.into();
        let amount_two: Amount = 1.into();

        let amounts = vec![amount_one, amount_two];

        let total = Amount::try_sum(amounts);

        assert!(total.is_err());
        let amount_one: Amount = 10000.into();
        let amount_two: Amount = 1.into();

        let amounts = vec![amount_one, amount_two];
        let total = Amount::try_sum(amounts).unwrap();

        assert_eq!(total, 10001.into());
    }

    #[test]
    fn test_amount_to_unit() {
        let amount = Amount::from(1000);
        let current_unit = CurrencyUnit::Sat;
        let target_unit = CurrencyUnit::Msat;

        let converted = to_unit(amount, &current_unit, &target_unit).unwrap();

        assert_eq!(converted, 1000000.into());

        let amount = Amount::from(1000);
        let current_unit = CurrencyUnit::Msat;
        let target_unit = CurrencyUnit::Sat;

        let converted = to_unit(amount, &current_unit, &target_unit).unwrap();

        assert_eq!(converted, 1.into());

        let amount = Amount::from(1);
        let current_unit = CurrencyUnit::Usd;
        let target_unit = CurrencyUnit::Usd;

        let converted = to_unit(amount, &current_unit, &target_unit).unwrap();

        assert_eq!(converted, 1.into());

        let amount = Amount::from(1);
        let current_unit = CurrencyUnit::Eur;
        let target_unit = CurrencyUnit::Eur;

        let converted = to_unit(amount, &current_unit, &target_unit).unwrap();

        assert_eq!(converted, 1.into());

        let amount = Amount::from(1);
        let current_unit = CurrencyUnit::Sat;
        let target_unit = CurrencyUnit::Eur;

        let converted = to_unit(amount, &current_unit, &target_unit);

        assert!(converted.is_err());
    }
}

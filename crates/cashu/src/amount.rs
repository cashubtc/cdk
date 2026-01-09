//! CDK Amount
//!
//! Is any unit and will be treated as the unit of the wallet

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use lightning::offers::offer::Offer;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::nuts::CurrencyUnit;
use crate::Id;

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
    /// Cannot perform operation on amounts with different units
    #[error("Unit mismatch: cannot operate on {0} and {1}")]
    UnitMismatch(CurrencyUnit, CurrencyUnit),
    /// Invalid amount
    #[error("Invalid Amount: {0}")]
    InvalidAmount(String),
    /// Amount undefined
    #[error("Amount undefined")]
    AmountUndefined,
    /// Utf8 parse error
    #[error(transparent)]
    Utf8ParseError(#[from] std::string::FromUtf8Error),
}

/// Amount can be any unit
///
/// Note: `PartialOrd` is implemented manually for `Amount<CurrencyUnit>` to return `None`
/// when comparing amounts with different units. `Ord` is only implemented for `Amount<()>`.
#[derive(Debug, Hash, PartialEq, Eq)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct Amount<U = ()> {
    value: u64,
    unit: U,
}

/// Fees and and amount type, it can be casted just as a reference to the inner amounts, or a single
/// u64 which is the fee
#[derive(Debug, Clone)]
pub struct FeeAndAmounts {
    fee: u64,
    amounts: Vec<u64>,
}

impl From<(u64, Vec<u64>)> for FeeAndAmounts {
    fn from(value: (u64, Vec<u64>)) -> Self {
        Self {
            fee: value.0,
            amounts: value.1,
        }
    }
}

impl FeeAndAmounts {
    /// Fees
    #[inline(always)]
    pub fn fee(&self) -> u64 {
        self.fee
    }

    /// Amounts
    #[inline(always)]
    pub fn amounts(&self) -> &[u64] {
        &self.amounts
    }
}

/// Fees and Amounts for each Keyset
pub type KeysetFeeAndAmounts = HashMap<Id, FeeAndAmounts>;

// Copy and Clone implementations for Amount<()>
impl Copy for Amount<()> {}

impl Clone for Amount<()> {
    fn clone(&self) -> Self {
        *self
    }
}

// Clone implementation for Amount<CurrencyUnit>
impl Clone for Amount<CurrencyUnit> {
    fn clone(&self) -> Self {
        Self {
            value: self.value,
            unit: self.unit.clone(),
        }
    }
}

// PartialOrd implementation for Amount<()> - always comparable
impl PartialOrd for Amount<()> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

// Ord implementation for Amount<()> - total ordering on value
impl Ord for Amount<()> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.value.cmp(&other.value)
    }
}

// PartialOrd implementation for Amount<CurrencyUnit> - returns None if units differ
impl PartialOrd for Amount<CurrencyUnit> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        if self.unit != other.unit {
            // Different units are not comparable
            None
        } else {
            Some(self.value.cmp(&other.value))
        }
    }
}

// Note: We intentionally do NOT implement Ord for Amount<CurrencyUnit>
// because amounts with different units cannot have a total ordering.

// Serialization - both variants serialize as just the u64 value
impl<U> Serialize for Amount<U> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.value.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Amount<()> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = u64::deserialize(deserializer)?;
        Ok(Amount { value, unit: () })
    }
}

impl FromStr for Amount<()> {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let value = s
            .parse::<u64>()
            .map_err(|_| Error::InvalidAmount(s.to_owned()))?;
        Ok(Amount { value, unit: () })
    }
}

impl Amount<()> {
    /// Amount zero
    pub const ZERO: Amount<()> = Amount { value: 0, unit: () };

    /// Amount one
    pub const ONE: Amount<()> = Amount { value: 1, unit: () };

    /// Convert an untyped amount to a typed one by adding a unit
    ///
    /// This is used at the boundary between protocol and application layers.
    /// Protocol types use `Amount<()>` (no unit), while application types
    /// use `Amount<CurrencyUnit>` (with unit from keyset).
    ///
    /// # Example
    /// ```
    /// # use cashu::{Amount, nuts::CurrencyUnit};
    /// let untyped = Amount::from(100);
    /// let typed = untyped.with_unit(CurrencyUnit::Sat);
    /// assert_eq!(typed.value(), 100);
    /// assert_eq!(typed.unit(), &CurrencyUnit::Sat);
    /// ```
    pub fn with_unit(self, unit: CurrencyUnit) -> Amount<CurrencyUnit> {
        Amount {
            value: self.value,
            unit,
        }
    }

    /// Split into parts that are powers of two
    pub fn split(&self, fee_and_amounts: &FeeAndAmounts) -> Vec<Self> {
        fee_and_amounts
            .amounts
            .iter()
            .rev()
            .fold((Vec::new(), self.value), |(mut acc, total), &amount| {
                if total >= amount {
                    acc.push(Self::from(amount));
                }
                (acc, total % amount)
            })
            .0
    }

    /// Split into parts that are powers of two by target
    pub fn split_targeted(
        &self,
        target: &SplitTarget,
        fee_and_amounts: &FeeAndAmounts,
    ) -> Result<Vec<Self>, Error> {
        let mut parts = match target {
            SplitTarget::None => self.split(fee_and_amounts),
            SplitTarget::Value(amount) => {
                if self.le(amount) {
                    return Ok(self.split(fee_and_amounts));
                }

                let mut parts_total = Amount::ZERO;
                let mut parts = Vec::new();

                // The powers of two that are need to create target value
                let parts_of_value = amount.split(fee_and_amounts);

                while parts_total.lt(self) {
                    for part in parts_of_value.iter().copied() {
                        if (part.checked_add(parts_total).ok_or(Error::AmountOverflow)?).le(self) {
                            parts.push(part);
                        } else {
                            let amount_left =
                                self.checked_sub(parts_total).ok_or(Error::AmountOverflow)?;
                            parts.extend(amount_left.split(fee_and_amounts));
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
                        let extra = self
                            .checked_sub(values_total)
                            .ok_or(Error::AmountOverflow)?;
                        let mut extra_amount = extra.split(fee_and_amounts);
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

    /// Splits amount into powers of two while accounting for the swap fee
    pub fn split_with_fee(&self, fee_and_amounts: &FeeAndAmounts) -> Result<Vec<Self>, Error> {
        let without_fee_amounts = self.split(fee_and_amounts);
        let total_fee_ppk = fee_and_amounts
            .fee
            .checked_mul(without_fee_amounts.len() as u64)
            .ok_or(Error::AmountOverflow)?;
        let fee = Amount::from(total_fee_ppk.div_ceil(1000));
        let new_amount = self.checked_add(fee).ok_or(Error::AmountOverflow)?;

        let split = new_amount.split(fee_and_amounts);
        let split_fee_ppk = (split.len() as u64)
            .checked_mul(fee_and_amounts.fee)
            .ok_or(Error::AmountOverflow)?;
        let split_fee = Amount::from(split_fee_ppk.div_ceil(1000));

        if let Some(net_amount) = new_amount.checked_sub(split_fee) {
            if net_amount >= *self {
                return Ok(split);
            }
        }
        self.checked_add(Amount::ONE)
            .ok_or(Error::AmountOverflow)?
            .split_with_fee(fee_and_amounts)
    }

    /// Checked addition for Amount. Returns None if overflow occurs.
    pub fn checked_add(self, other: Amount<()>) -> Option<Amount<()>> {
        self.value
            .checked_add(other.value)
            .map(|v| Amount { value: v, unit: () })
    }

    /// Checked subtraction for Amount. Returns None if overflow occurs.
    pub fn checked_sub(self, other: Amount<()>) -> Option<Amount<()>> {
        self.value
            .checked_sub(other.value)
            .map(|v| Amount { value: v, unit: () })
    }

    /// Checked multiplication for Amount. Returns None if overflow occurs.
    pub fn checked_mul(self, other: Amount<()>) -> Option<Amount<()>> {
        self.value
            .checked_mul(other.value)
            .map(|v| Amount { value: v, unit: () })
    }

    /// Checked division for Amount. Returns None if overflow occurs.
    pub fn checked_div(self, other: Amount<()>) -> Option<Amount<()>> {
        self.value
            .checked_div(other.value)
            .map(|v| Amount { value: v, unit: () })
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

    /// Convert unit
    pub fn convert_unit(
        &self,
        current_unit: &CurrencyUnit,
        target_unit: &CurrencyUnit,
    ) -> Result<Amount<()>, Error> {
        Amount::new(self.value, current_unit.clone())
            .convert_to(target_unit)
            .map(Into::into)
    }

    /// Convert to u64
    pub fn to_u64(self) -> u64 {
        self.value
    }

    /// Convert to i64
    pub fn to_i64(self) -> Option<i64> {
        if self.value <= i64::MAX as u64 {
            Some(self.value as i64)
        } else {
            None
        }
    }

    /// Create from i64, returning None if negative
    pub fn from_i64(value: i64) -> Option<Self> {
        if value >= 0 {
            Some(Amount {
                value: value as u64,
                unit: (),
            })
        } else {
            None
        }
    }
}

impl Default for Amount<()> {
    fn default() -> Self {
        Amount::ZERO
    }
}

impl Default for &Amount<()> {
    fn default() -> Self {
        &Amount::ZERO
    }
}

impl Amount<CurrencyUnit> {
    /// Create a new Amount with an explicit unit
    ///
    /// This is the primary constructor for typed amounts. It works with all
    /// CurrencyUnit variants including Custom.
    ///
    /// # Example
    /// ```
    /// # use cashu::{Amount, nuts::CurrencyUnit};
    /// let sat_amount = Amount::new(1000, CurrencyUnit::Sat);
    /// let custom = Amount::new(50, CurrencyUnit::Custom("BTC".into()));
    /// ```
    pub fn new(value: u64, unit: CurrencyUnit) -> Self {
        Self { value, unit }
    }

    /// Get the numeric value
    ///
    /// # Example
    /// ```
    /// # use cashu::{Amount, nuts::CurrencyUnit};
    /// let amount = Amount::new(1000, CurrencyUnit::Sat);
    /// assert_eq!(amount.value(), 1000);
    /// ```
    pub fn value(&self) -> u64 {
        self.value
    }

    /// Convert to u64
    pub fn to_u64(self) -> u64 {
        self.value
    }

    /// Convert to i64
    pub fn to_i64(self) -> Option<i64> {
        if self.value <= i64::MAX as u64 {
            Some(self.value as i64)
        } else {
            None
        }
    }

    /// Get a reference to the unit
    ///
    /// # Example
    /// ```
    /// # use cashu::{Amount, nuts::CurrencyUnit};
    /// let amount = Amount::new(1000, CurrencyUnit::Sat);
    /// assert_eq!(amount.unit(), &CurrencyUnit::Sat);
    /// ```
    pub fn unit(&self) -> &CurrencyUnit {
        &self.unit
    }

    /// Consume self and return both value and unit
    ///
    /// # Example
    /// ```
    /// # use cashu::{Amount, nuts::CurrencyUnit};
    /// let amount = Amount::new(1000, CurrencyUnit::Sat);
    /// let (value, unit) = amount.into_parts();
    /// assert_eq!(value, 1000);
    /// assert_eq!(unit, CurrencyUnit::Sat);
    /// ```
    pub fn into_parts(self) -> (u64, CurrencyUnit) {
        (self.value, self.unit)
    }

    /// Checked addition with unit verification
    ///
    /// Returns an error if units don't match or if overflow occurs.
    ///
    /// # Example
    /// ```
    /// # use cashu::{Amount, nuts::CurrencyUnit};
    /// let a = Amount::new(100, CurrencyUnit::Sat);
    /// let b = Amount::new(50, CurrencyUnit::Sat);
    /// let sum = a.checked_add(&b).unwrap();
    /// assert_eq!(sum.value(), 150);
    ///
    /// // Different units cause an error
    /// let c = Amount::new(100, CurrencyUnit::Msat);
    /// assert!(a.checked_add(&c).is_err());
    /// ```
    pub fn checked_add(&self, other: &Self) -> Result<Self, Error> {
        if self.unit != other.unit {
            return Err(Error::UnitMismatch(self.unit.clone(), other.unit.clone()));
        }
        self.value
            .checked_add(other.value)
            .map(|v| Amount::new(v, self.unit.clone()))
            .ok_or(Error::AmountOverflow)
    }

    /// Checked subtraction with unit verification
    ///
    /// Returns an error if units don't match or if underflow occurs.
    ///
    /// # Example
    /// ```
    /// # use cashu::{Amount, nuts::CurrencyUnit};
    /// let a = Amount::new(100, CurrencyUnit::Sat);
    /// let b = Amount::new(30, CurrencyUnit::Sat);
    /// let diff = a.checked_sub(&b).unwrap();
    /// assert_eq!(diff.value(), 70);
    /// ```
    pub fn checked_sub(&self, other: &Self) -> Result<Self, Error> {
        if self.unit != other.unit {
            return Err(Error::UnitMismatch(self.unit.clone(), other.unit.clone()));
        }
        self.value
            .checked_sub(other.value)
            .map(|v| Amount::new(v, self.unit.clone()))
            .ok_or(Error::AmountOverflow)
    }

    /// Convert to a different unit
    ///
    /// # Example
    /// ```
    /// # use cashu::{Amount, nuts::CurrencyUnit};
    /// let sat = Amount::new(1000, CurrencyUnit::Sat);
    /// let msat = sat.convert_to(&CurrencyUnit::Msat).unwrap();
    /// assert_eq!(msat.value(), 1_000_000);
    /// assert_eq!(msat.unit(), &CurrencyUnit::Msat);
    /// ```
    pub fn convert_to(&self, target_unit: &CurrencyUnit) -> Result<Self, Error> {
        if &self.unit == target_unit {
            return Ok(self.clone());
        }

        let converted_value = match (&self.unit, target_unit) {
            (CurrencyUnit::Sat, CurrencyUnit::Msat) => self
                .value
                .checked_mul(MSAT_IN_SAT)
                .ok_or(Error::AmountOverflow)?,
            (CurrencyUnit::Msat, CurrencyUnit::Sat) => self.value / MSAT_IN_SAT,
            _ => return Err(Error::CannotConvertUnits),
        };

        Ok(Amount::new(converted_value, target_unit.clone()))
    }

    /// Returns a string representation that includes the unit
    pub fn display_with_unit(&self) -> String {
        format!("{} {}", self.value, self.unit)
    }
}

impl<U> fmt::Display for Amount<U> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(width) = f.width() {
            write!(f, "{:width$}", self.value, width = width)
        } else {
            write!(f, "{}", self.value)
        }
    }
}

impl From<u64> for Amount<()> {
    fn from(value: u64) -> Self {
        Amount { value, unit: () }
    }
}

impl From<&u64> for Amount<()> {
    fn from(value: &u64) -> Self {
        Amount {
            value: *value,
            unit: (),
        }
    }
}

impl From<Amount<()>> for u64 {
    fn from(value: Amount<()>) -> Self {
        value.value
    }
}

impl From<Amount<CurrencyUnit>> for Amount<()> {
    fn from(value: Amount<CurrencyUnit>) -> Self {
        Amount {
            value: value.value,
            unit: (),
        }
    }
}

impl AsRef<u64> for Amount<()> {
    fn as_ref(&self) -> &u64 {
        &self.value
    }
}

impl std::ops::Add for Amount<()> {
    type Output = Amount<()>;

    fn add(self, rhs: Amount<()>) -> Self::Output {
        self.checked_add(rhs)
            .expect("Addition overflow: the sum of the amounts exceeds the maximum value")
    }
}

impl std::ops::AddAssign for Amount<()> {
    fn add_assign(&mut self, rhs: Self) {
        *self = self
            .checked_add(rhs)
            .expect("AddAssign overflow: the sum of the amounts exceeds the maximum value");
    }
}

impl std::ops::Sub for Amount<()> {
    type Output = Amount<()>;

    fn sub(self, rhs: Amount<()>) -> Self::Output {
        self.checked_sub(rhs)
            .expect("Subtraction underflow: cannot subtract a larger amount from a smaller amount")
    }
}

impl std::ops::SubAssign for Amount<()> {
    fn sub_assign(&mut self, other: Self) {
        *self = self
            .checked_sub(other)
            .expect("SubAssign underflow: cannot subtract a larger amount from a smaller amount");
    }
}

impl std::ops::Mul for Amount<()> {
    type Output = Self;

    fn mul(self, other: Self) -> Self::Output {
        self.checked_mul(other)
            .expect("Multiplication overflow: the product of the amounts exceeds the maximum value")
    }
}

impl std::ops::Div for Amount<()> {
    type Output = Self;

    fn div(self, other: Self) -> Self::Output {
        self.checked_div(other)
            .expect("Division error: cannot divide by zero or overflow occurred")
    }
}

/// Convert offer to amount in unit
pub fn amount_for_offer(offer: &Offer, unit: &CurrencyUnit) -> Result<Amount, Error> {
    let offer_amount = offer.amount().ok_or(Error::AmountUndefined)?;

    let (amount, currency) = match offer_amount {
        lightning::offers::offer::Amount::Bitcoin { amount_msats } => {
            (amount_msats, CurrencyUnit::Msat)
        }
        lightning::offers::offer::Amount::Currency {
            iso4217_code,
            amount,
        } => (
            amount,
            CurrencyUnit::from_str(&String::from_utf8(iso4217_code.as_bytes().to_vec())?)
                .map_err(|_| Error::CannotConvertUnits)?,
        ),
    };

    Amount::new(amount, currency)
        .convert_to(unit)
        .map(Into::into)
        .map_err(|_err| Error::CannotConvertUnits)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_amount() {
        let fee_and_amounts = (0, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into();

        assert_eq!(
            Amount::from(1).split(&fee_and_amounts),
            vec![Amount::from(1)]
        );
        assert_eq!(
            Amount::from(2).split(&fee_and_amounts),
            vec![Amount::from(2)]
        );
        assert_eq!(
            Amount::from(3).split(&fee_and_amounts),
            vec![Amount::from(2), Amount::from(1)]
        );
        let amounts: Vec<Amount> = [8, 2, 1].iter().map(|a| Amount::from(*a)).collect();
        assert_eq!(Amount::from(11).split(&fee_and_amounts), amounts);
        let amounts: Vec<Amount> = [128, 64, 32, 16, 8, 4, 2, 1]
            .iter()
            .map(|a| Amount::from(*a))
            .collect();
        assert_eq!(Amount::from(255).split(&fee_and_amounts), amounts);
    }

    #[test]
    fn test_split_target_amount() {
        let fee_and_amounts = (0, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into();
        let amount = Amount::from(65);

        let split = amount
            .split_targeted(&SplitTarget::Value(Amount::from(32)), &fee_and_amounts)
            .unwrap();
        assert_eq!(
            vec![Amount::from(1), Amount::from(32), Amount::from(32)],
            split
        );

        let amount = Amount::from(150);

        let split = amount
            .split_targeted(&SplitTarget::Value(Amount::from(50)), &fee_and_amounts)
            .unwrap();
        assert_eq!(
            vec![
                Amount::from(2),
                Amount::from(2),
                Amount::from(2),
                Amount::from(16),
                Amount::from(16),
                Amount::from(16),
                Amount::from(32),
                Amount::from(32),
                Amount::from(32)
            ],
            split
        );

        let amount = Amount::from(63);

        let split = amount
            .split_targeted(&SplitTarget::Value(Amount::from(32)), &fee_and_amounts)
            .unwrap();
        assert_eq!(
            vec![
                Amount::from(1),
                Amount::from(2),
                Amount::from(4),
                Amount::from(8),
                Amount::from(16),
                Amount::from(32)
            ],
            split
        );
    }

    #[test]
    fn test_split_with_fee() {
        let fee_and_amounts = (1, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into();
        let amount = Amount::from(2);

        let split = amount.split_with_fee(&fee_and_amounts).unwrap();
        assert_eq!(split, vec![Amount::from(2), Amount::from(1)]);

        let amount = Amount::from(3);

        let split = amount.split_with_fee(&fee_and_amounts).unwrap();
        assert_eq!(split, vec![Amount::from(4)]);

        let amount = Amount::from(3);
        let fee_and_amounts = (1000, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into();

        let split = amount.split_with_fee(&fee_and_amounts).unwrap();
        // With fee_ppk=1000 (100%), amount 3 requires proofs totaling at least 5
        // to cover both the amount (3) and fees (~2 for 2 proofs)
        assert_eq!(split, vec![Amount::from(4), Amount::from(1)]);
    }

    #[test]
    fn test_split_with_fee_reported_issue() {
        let fee_and_amounts = (100, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into();
        // Test the reported issue: mint 600, send 300 with fee_ppk=100
        let amount = Amount::from(300);

        let split = amount.split_with_fee(&fee_and_amounts).unwrap();

        // Calculate the total fee for the split
        let total_fee_ppk = (split.len() as u64) * fee_and_amounts.fee;
        let total_fee = Amount::from(total_fee_ppk.div_ceil(1000));

        // The split should cover the amount plus fees
        let split_total = Amount::try_sum(split.iter().copied()).unwrap();
        assert!(
            split_total >= amount.checked_add(total_fee).unwrap(),
            "Split total {} should be >= amount {} + fee {}",
            split_total,
            amount,
            total_fee
        );
    }

    #[test]
    fn test_split_with_fee_edge_cases() {
        // Test various amounts with fee_ppk=100
        let test_cases = vec![
            (Amount::from(1), 100),
            (Amount::from(10), 100),
            (Amount::from(50), 100),
            (Amount::from(100), 100),
            (Amount::from(200), 100),
            (Amount::from(300), 100),
            (Amount::from(500), 100),
            (Amount::from(600), 100),
            (Amount::from(1000), 100),
            (Amount::from(1337), 100),
            (Amount::from(5000), 100),
        ];

        for (amount, fee_ppk) in test_cases {
            let fee_and_amounts =
                (fee_ppk, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into();
            let result = amount.split_with_fee(&fee_and_amounts);
            assert!(
                result.is_ok(),
                "split_with_fee failed for amount {} with fee_ppk {}: {:?}",
                amount,
                fee_ppk,
                result.err()
            );

            let split = result.unwrap();

            // Verify the split covers the required amount
            let split_total = Amount::try_sum(split.iter().copied()).unwrap();
            let fee_for_split = (split.len() as u64) * fee_ppk;
            let total_fee = Amount::from(fee_for_split.div_ceil(1000));

            // The net amount after fees should be at least the original amount
            let net_amount = split_total.checked_sub(total_fee);
            assert!(
                net_amount.is_some(),
                "Net amount calculation failed for amount {} with fee_ppk {}",
                amount,
                fee_ppk
            );
            assert!(
                net_amount.unwrap() >= amount,
                "Net amount {} is less than required {} for amount {} with fee_ppk {}",
                net_amount.unwrap(),
                amount,
                amount,
                fee_ppk
            );
        }
    }

    #[test]
    fn test_split_with_fee_high_fees() {
        // Test with very high fees
        let test_cases = vec![
            (Amount::from(10), 500),  // 50% fee
            (Amount::from(10), 1000), // 100% fee
            (Amount::from(10), 2000), // 200% fee
            (Amount::from(100), 500),
            (Amount::from(100), 1000),
            (Amount::from(100), 2000),
        ];

        for (amount, fee_ppk) in test_cases {
            let fee_and_amounts =
                (fee_ppk, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into();
            let result = amount.split_with_fee(&fee_and_amounts);
            assert!(
                result.is_ok(),
                "split_with_fee failed for amount {} with fee_ppk {}: {:?}",
                amount,
                fee_ppk,
                result.err()
            );

            let split = result.unwrap();
            let split_total = Amount::try_sum(split.iter().copied()).unwrap();

            // With high fees, we just need to ensure we can cover the amount
            assert!(
                split_total > amount,
                "Split total {} should be greater than amount {} for fee_ppk {}",
                split_total,
                amount,
                fee_ppk
            );
        }
    }

    #[test]
    fn test_split_with_fee_recursion_limit() {
        // Test that the recursion doesn't go infinite
        // This tests the edge case where the method keeps adding Amount::ONE
        let amount = Amount::from(1);
        let fee_ppk = 10000;
        let fee_and_amounts = (fee_ppk, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into();

        let result = amount.split_with_fee(&fee_and_amounts);
        assert!(
            result.is_ok(),
            "split_with_fee should handle extreme fees without infinite recursion"
        );
    }

    #[test]
    fn test_split_values() {
        let fee_and_amounts = (0, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into();
        let amount = Amount::from(10);

        let target = vec![Amount::from(2), Amount::from(4), Amount::from(4)];

        let split_target = SplitTarget::Values(target.clone());

        let values = amount
            .split_targeted(&split_target, &fee_and_amounts)
            .unwrap();

        assert_eq!(target, values);

        let target = vec![Amount::from(2), Amount::from(4), Amount::from(4)];

        let split_target = SplitTarget::Values(vec![Amount::from(2), Amount::from(4)]);

        let values = amount
            .split_targeted(&split_target, &fee_and_amounts)
            .unwrap();

        assert_eq!(target, values);

        let split_target = SplitTarget::Values(vec![Amount::from(2), Amount::from(10)]);

        let values = amount.split_targeted(&split_target, &fee_and_amounts);

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
    fn test_amount_convert_to() {
        // Sat -> Msat
        let amount = Amount::new(1000, CurrencyUnit::Sat);
        let converted = amount.convert_to(&CurrencyUnit::Msat).unwrap();
        assert_eq!(converted.value(), 1000000);
        assert_eq!(converted.unit(), &CurrencyUnit::Msat);

        // Msat -> Sat
        let amount = Amount::new(1000, CurrencyUnit::Msat);
        let converted = amount.convert_to(&CurrencyUnit::Sat).unwrap();
        assert_eq!(converted.value(), 1);
        assert_eq!(converted.unit(), &CurrencyUnit::Sat);

        // Usd -> Usd identity conversion
        let amount = Amount::new(1, CurrencyUnit::Usd);
        let converted = amount.convert_to(&CurrencyUnit::Usd).unwrap();
        assert_eq!(converted.value(), 1);
        assert_eq!(converted.unit(), &CurrencyUnit::Usd);

        // Eur -> Eur identity conversion
        let amount = Amount::new(1, CurrencyUnit::Eur);
        let converted = amount.convert_to(&CurrencyUnit::Eur).unwrap();
        assert_eq!(converted.value(), 1);
        assert_eq!(converted.unit(), &CurrencyUnit::Eur);

        // Sat -> Eur should fail (no conversion path)
        let amount = Amount::new(1, CurrencyUnit::Sat);
        let converted = amount.convert_to(&CurrencyUnit::Eur);
        assert!(converted.is_err());

        // Sat -> Sat identity conversion
        let amount = Amount::new(500, CurrencyUnit::Sat);
        let converted = amount.convert_to(&CurrencyUnit::Sat).unwrap();
        assert_eq!(converted.value(), 500);
        assert_eq!(converted.unit(), &CurrencyUnit::Sat);

        // Msat -> Msat identity conversion
        let amount = Amount::new(5000, CurrencyUnit::Msat);
        let converted = amount.convert_to(&CurrencyUnit::Msat).unwrap();
        assert_eq!(converted.value(), 5000);
        assert_eq!(converted.unit(), &CurrencyUnit::Msat);
    }

    #[test]
    fn test_amount_from_typed_to_untyped() {
        // Test From<Amount<CurrencyUnit>> for Amount<()>
        let typed = Amount::new(1000, CurrencyUnit::Sat);
        let untyped: Amount<()> = typed.into();
        assert_eq!(u64::from(untyped), 1000);
    }

    /// Tests that the subtraction operator correctly computes the difference between amounts.
    ///
    /// This test verifies that the `-` operator for Amount produces the expected result.
    /// It's particularly important because the subtraction operation is used in critical
    /// code paths like `split_targeted`, where incorrect subtraction could lead to
    /// infinite loops or wrong calculations.
    ///
    /// Mutant testing: Catches mutations that replace the subtraction implementation
    /// with `Default::default()` (returning Amount::ZERO), which would cause infinite
    /// loops in `split_targeted` at line 138 where `*self - parts_total` is computed.
    #[test]
    fn test_amount_sub_operator() {
        let amount1 = Amount::from(100);
        let amount2 = Amount::from(30);

        let result = amount1.checked_sub(amount2).unwrap();
        assert_eq!(result, Amount::from(70));

        let amount1 = Amount::from(1000);
        let amount2 = Amount::from(1);

        let result = amount1.checked_sub(amount2).unwrap();
        assert_eq!(result, Amount::from(999));

        let amount1 = Amount::from(255);
        let amount2 = Amount::from(128);

        let result = amount1.checked_sub(amount2).unwrap();
        assert_eq!(result, Amount::from(127));
    }

    /// Tests that the subtraction operator panics when attempting to subtract
    /// a larger amount from a smaller amount (underflow).
    ///
    /// This test verifies the safety property that Amount subtraction will panic
    /// rather than wrap around on underflow. This is critical for preventing
    /// bugs where negative amounts could be interpreted as very large positive amounts.
    ///
    /// Mutant testing: Catches mutations that remove the panic behavior or return
    /// default values instead of properly handling underflow.
    #[test]
    #[should_panic(expected = "Subtraction underflow")]
    fn test_amount_sub_underflow() {
        let amount1 = Amount::from(30);
        let amount2 = Amount::from(100);

        let _result = amount1 - amount2;
    }

    /// Tests that checked_add correctly computes the sum and returns the actual value.
    ///
    /// This is critical because checked_add is used in recursive functions like
    /// split_with_fee. If it returns Some(Amount::ZERO) instead of the actual sum,
    /// the recursion would never terminate.
    ///
    /// Mutant testing: Kills mutations that replace the implementation with
    /// `Some(Default::default())`, which would cause infinite loops in split_with_fee
    /// at line 198 where it recursively calls itself with incremented amounts.
    #[test]
    fn test_checked_add_returns_correct_value() {
        let amount1 = Amount::from(100);
        let amount2 = Amount::from(50);

        let result = amount1.checked_add(amount2);
        assert_eq!(result, Some(Amount::from(150)));

        let amount1 = Amount::from(1);
        let amount2 = Amount::from(1);

        let result = amount1.checked_add(amount2);
        assert_eq!(result, Some(Amount::from(2)));
        assert_ne!(result, Some(Amount::ZERO));

        let amount1 = Amount::from(1000);
        let amount2 = Amount::from(337);

        let result = amount1.checked_add(amount2);
        assert_eq!(result, Some(Amount::from(1337)));
    }

    /// Tests that checked_add returns None on overflow.
    #[test]
    fn test_checked_add_overflow() {
        let amount1 = Amount::from(u64::MAX);
        let amount2 = Amount::from(1);

        let result = amount1.checked_add(amount2);
        assert!(result.is_none());
    }

    /// Tests that try_sum correctly computes the total sum of amounts.
    ///
    /// This is critical because try_sum is used in loops like split_targeted at line 130
    /// to track progress. If it returns Ok(Amount::ZERO) instead of the actual sum,
    /// the loop condition `parts_total.eq(self)` would never be true, causing an infinite loop.
    ///
    /// Mutant testing: Kills mutations that replace the implementation with
    /// `Ok(Default::default())`, which would cause infinite loops.
    #[test]
    fn test_try_sum_returns_correct_value() {
        let amounts = vec![Amount::from(10), Amount::from(20), Amount::from(30)];
        let result = Amount::try_sum(amounts).unwrap();
        assert_eq!(result, Amount::from(60));
        assert_ne!(result, Amount::ZERO);

        let amounts = vec![Amount::from(1), Amount::from(1), Amount::from(1)];
        let result = Amount::try_sum(amounts).unwrap();
        assert_eq!(result, Amount::from(3));

        let amounts = vec![Amount::from(100)];
        let result = Amount::try_sum(amounts).unwrap();
        assert_eq!(result, Amount::from(100));

        let empty: Vec<Amount> = vec![];
        let result = Amount::try_sum(empty).unwrap();
        assert_eq!(result, Amount::ZERO);
    }

    /// Tests that try_sum returns error on overflow.
    #[test]
    fn test_try_sum_overflow() {
        let amounts = vec![Amount::from(u64::MAX), Amount::from(1)];
        let result = Amount::try_sum(amounts);
        assert!(result.is_err());
    }

    /// Tests that split returns a non-empty vec with actual values, not defaults.
    ///
    /// The split function is used in split_targeted's while loop (line 122).
    /// If split returns an empty vec or vec with Amount::ZERO when it shouldn't,
    /// the loop that extends parts with split results would never make progress,
    /// causing an infinite loop.
    ///
    /// Mutant testing: Kills mutations that replace split with `vec![]` or
    /// `vec![Default::default()]` which would cause infinite loops.
    #[test]
    fn test_split_returns_correct_values() {
        let fee_and_amounts = (0, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into();

        let amount = Amount::from(11);
        let result = amount.split(&fee_and_amounts);
        assert!(!result.is_empty());
        assert_eq!(Amount::try_sum(result.iter().copied()).unwrap(), amount);

        let amount = Amount::from(255);
        let result = amount.split(&fee_and_amounts);
        assert!(!result.is_empty());
        assert_eq!(Amount::try_sum(result.iter().copied()).unwrap(), amount);

        let amount = Amount::from(7);
        let result = amount.split(&fee_and_amounts);
        assert_eq!(
            result,
            vec![Amount::from(4), Amount::from(2), Amount::from(1)]
        );
        for r in &result {
            assert_ne!(*r, Amount::ZERO);
        }
    }

    /// Tests that the modulo operation in split works correctly.
    ///
    /// At line 108, split uses modulo (%) to compute the remainder.
    /// If this is mutated to division (/), it would produce wrong results
    /// that could cause infinite loops in code that depends on split.
    ///
    /// Mutant testing: Kills mutations that replace `%` with `/`.
    #[test]
    fn test_split_modulo_operation() {
        let fee_and_amounts = (0, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into();

        let amount = Amount::from(15);
        let result = amount.split(&fee_and_amounts);

        assert_eq!(
            result,
            vec![
                Amount::from(8),
                Amount::from(4),
                Amount::from(2),
                Amount::from(1)
            ]
        );

        let total = Amount::try_sum(result.iter().copied()).unwrap();
        assert_eq!(total, amount);
    }

    /// Tests that From<u64> correctly converts values to Amount.
    ///
    /// This conversion is used throughout the codebase including in loops and split operations.
    /// If it returns Default::default() (Amount::ZERO) instead of the actual value,
    /// it can cause infinite loops where amounts are being accumulated or compared.
    ///
    /// Mutant testing: Kills mutations that replace From<u64> with `Default::default()`.
    #[test]
    fn test_from_u64_returns_correct_value() {
        let amount = Amount::from(100u64);
        assert_eq!(amount, Amount::from(100));
        assert_ne!(amount, Amount::ZERO);

        let amount = Amount::from(1u64);
        assert_eq!(amount, Amount::from(1));
        assert_eq!(amount, Amount::ONE);

        let amount = Amount::from(1337u64);
        assert_eq!(amount.to_u64(), 1337);
    }

    /// Tests that checked_mul returns the correct product value.
    ///
    /// This is critical for any multiplication operations. If it returns None
    /// or Some(Amount::ZERO) instead of the actual product, calculations will be wrong.
    ///
    /// Mutant testing: Kills mutations that replace checked_mul with None or Some(Default::default()).
    #[test]
    fn test_checked_mul_returns_correct_value() {
        let amount1 = Amount::from(10);
        let amount2 = Amount::from(5);
        let result = amount1.checked_mul(amount2);
        assert_eq!(result, Some(Amount::from(50)));
        assert_ne!(result, None);
        assert_ne!(result, Some(Amount::ZERO));

        let amount1 = Amount::from(100);
        let amount2 = Amount::from(20);
        let result = amount1.checked_mul(amount2);
        assert_eq!(result, Some(Amount::from(2000)));
        assert_ne!(result, Some(Amount::ZERO));

        let amount1 = Amount::from(7);
        let amount2 = Amount::from(13);
        let result = amount1.checked_mul(amount2);
        assert_eq!(result, Some(Amount::from(91)));

        // Test multiplication by zero
        let amount1 = Amount::from(100);
        let amount2 = Amount::ZERO;
        let result = amount1.checked_mul(amount2);
        assert_eq!(result, Some(Amount::ZERO));

        // Test multiplication by one
        let amount1 = Amount::from(42);
        let amount2 = Amount::ONE;
        let result = amount1.checked_mul(amount2);
        assert_eq!(result, Some(Amount::from(42)));

        // Test overflow
        let amount1 = Amount::from(u64::MAX);
        let amount2 = Amount::from(2);
        let result = amount1.checked_mul(amount2);
        assert!(result.is_none());
    }

    /// Tests that checked_div returns the correct quotient value.
    ///
    /// This is critical for division operations. If it returns None or
    /// Some(Amount::ZERO) instead of the actual quotient, calculations will be wrong.
    ///
    /// Mutant testing: Kills mutations that replace checked_div with None or Some(Default::default()).
    #[test]
    fn test_checked_div_returns_correct_value() {
        let amount1 = Amount::from(100);
        let amount2 = Amount::from(5);
        let result = amount1.checked_div(amount2);
        assert_eq!(result, Some(Amount::from(20)));
        assert_ne!(result, None);
        assert_ne!(result, Some(Amount::ZERO));

        let amount1 = Amount::from(1000);
        let amount2 = Amount::from(10);
        let result = amount1.checked_div(amount2);
        assert_eq!(result, Some(Amount::from(100)));
        assert_ne!(result, Some(Amount::ZERO));

        let amount1 = Amount::from(91);
        let amount2 = Amount::from(7);
        let result = amount1.checked_div(amount2);
        assert_eq!(result, Some(Amount::from(13)));

        // Test division by one
        let amount1 = Amount::from(42);
        let amount2 = Amount::ONE;
        let result = amount1.checked_div(amount2);
        assert_eq!(result, Some(Amount::from(42)));

        // Test integer division (truncation)
        let amount1 = Amount::from(10);
        let amount2 = Amount::from(3);
        let result = amount1.checked_div(amount2);
        assert_eq!(result, Some(Amount::from(3)));

        // Test division by zero
        let amount1 = Amount::from(100);
        let amount2 = Amount::ZERO;
        let result = amount1.checked_div(amount2);
        assert!(result.is_none());
    }

    /// Tests that Amount::convert_unit returns the correct converted value.
    ///
    /// This is critical for unit conversions. If it returns Ok(Amount::ZERO)
    /// instead of the actual converted value, all conversions will be wrong.
    ///
    /// Mutant testing: Kills mutations that replace convert_unit with Ok(Default::default()).
    #[test]
    fn test_convert_unit_returns_correct_value() {
        let amount = Amount::from(1000);
        let result = amount
            .convert_unit(&CurrencyUnit::Sat, &CurrencyUnit::Msat)
            .unwrap();
        assert_eq!(result, Amount::from(1_000_000));
        assert_ne!(result, Amount::ZERO);

        let amount = Amount::from(5000);
        let result = amount
            .convert_unit(&CurrencyUnit::Msat, &CurrencyUnit::Sat)
            .unwrap();
        assert_eq!(result, Amount::from(5));
        assert_ne!(result, Amount::ZERO);

        let amount = Amount::from(123);
        let result = amount
            .convert_unit(&CurrencyUnit::Sat, &CurrencyUnit::Sat)
            .unwrap();
        assert_eq!(result, Amount::from(123));

        let amount = Amount::from(456);
        let result = amount
            .convert_unit(&CurrencyUnit::Usd, &CurrencyUnit::Usd)
            .unwrap();
        assert_eq!(result, Amount::from(456));

        let amount = Amount::from(789);
        let result = amount
            .convert_unit(&CurrencyUnit::Eur, &CurrencyUnit::Eur)
            .unwrap();
        assert_eq!(result, Amount::from(789));

        // Test invalid conversion
        let amount = Amount::from(100);
        let result = amount.convert_unit(&CurrencyUnit::Sat, &CurrencyUnit::Eur);
        assert!(result.is_err());
    }

    /// Tests that Amount::to_i64() returns the correct value.
    ///
    /// Mutant testing: Kills mutations that replace the return value with:
    /// - None
    /// - Some(0)
    /// - Some(1)
    /// - Some(-1)
    /// Also catches mutation that replaces <= with > in the comparison.
    #[test]
    fn test_amount_to_i64_returns_correct_value() {
        // Test with value 100 (catches None, Some(0), Some(1), Some(-1) mutations)
        let amount = Amount::from(100);
        let result = amount.to_i64();
        assert_eq!(result, Some(100));
        assert!(result.is_some());
        assert_ne!(result, Some(0));
        assert_ne!(result, Some(1));
        assert_ne!(result, Some(-1));

        // Test with value 1000 (catches all constant mutations)
        let amount = Amount::from(1000);
        let result = amount.to_i64();
        assert_eq!(result, Some(1000));
        assert_ne!(result, None);
        assert_ne!(result, Some(0));
        assert_ne!(result, Some(1));
        assert_ne!(result, Some(-1));

        // Test with value 2 (specifically catches Some(1) mutation)
        let amount = Amount::from(2);
        let result = amount.to_i64();
        assert_eq!(result, Some(2));
        assert_ne!(result, Some(1));

        // Test with i64::MAX (should return Some(i64::MAX))
        // This catches the <= vs > mutation: if <= becomes >, this would return None
        let amount = Amount::from(i64::MAX as u64);
        let result = amount.to_i64();
        assert_eq!(result, Some(i64::MAX));
        assert!(result.is_some());

        // Test with i64::MAX + 1 (should return None)
        // This is the boundary case for the <= comparison
        let amount = Amount::from(i64::MAX as u64 + 1);
        let result = amount.to_i64();
        assert!(result.is_none());

        // Test with u64::MAX (should return None)
        let amount = Amount::from(u64::MAX);
        let result = amount.to_i64();
        assert!(result.is_none());

        // Edge case: 0 should return Some(0)
        let amount = Amount::from(0);
        let result = amount.to_i64();
        assert_eq!(result, Some(0));

        // Edge case: 1 should return Some(1)
        let amount = Amount::from(1);
        let result = amount.to_i64();
        assert_eq!(result, Some(1));
    }

    /// Tests the boundary condition for Amount::to_i64() at i64::MAX.
    ///
    /// This specifically tests the <= vs > mutation in the condition
    /// `if self.0 <= i64::MAX as u64`.
    #[test]
    fn test_amount_to_i64_boundary() {
        // Exactly at i64::MAX - should succeed
        let at_max = Amount::from(i64::MAX as u64);
        assert!(at_max.to_i64().is_some());
        assert_eq!(at_max.to_i64().unwrap(), i64::MAX);

        // One above i64::MAX - should fail
        let above_max = Amount::from(i64::MAX as u64 + 1);
        assert!(above_max.to_i64().is_none());

        // One below i64::MAX - should succeed
        let below_max = Amount::from(i64::MAX as u64 - 1);
        assert!(below_max.to_i64().is_some());
        assert_eq!(below_max.to_i64().unwrap(), i64::MAX - 1);
    }

    /// Tests Amount::from_i64 returns the correct value.
    ///
    /// Mutant testing: Catches mutations that:
    /// - Replace return with None
    /// - Replace return with Some(Default::default())
    /// - Replace >= with < in the condition
    #[test]
    fn test_amount_from_i64() {
        // Positive value - should return Some with correct value
        let result = Amount::from_i64(100);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), Amount::from(100));
        assert_ne!(result, None);
        assert_ne!(result, Some(Amount::ZERO));

        // Zero - boundary case for >= vs <
        // If >= becomes <, this would return None instead of Some
        let result = Amount::from_i64(0);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), Amount::ZERO);

        // Negative value - should return None
        let result = Amount::from_i64(-1);
        assert!(result.is_none());

        let result = Amount::from_i64(-100);
        assert!(result.is_none());

        // Large positive value
        let result = Amount::from_i64(i64::MAX);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), Amount::from(i64::MAX as u64));
        assert_ne!(result, Some(Amount::ZERO));

        // Value 1 - catches Some(Default::default()) mutation
        let result = Amount::from_i64(1);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), Amount::ONE);
        assert_ne!(result, Some(Amount::ZERO));
    }

    /// Tests AddAssign actually modifies the value.
    ///
    /// Mutant testing: Catches mutation that replaces add_assign with ().
    #[test]
    fn test_add_assign() {
        let mut amount = Amount::from(100);
        amount += Amount::from(50);
        assert_eq!(amount, Amount::from(150));
        assert_ne!(amount, Amount::from(100)); // Should have changed

        let mut amount = Amount::from(1);
        amount += Amount::from(1);
        assert_eq!(amount, Amount::from(2));
        assert_ne!(amount, Amount::ONE); // Should have changed

        let mut amount = Amount::ZERO;
        amount += Amount::from(42);
        assert_eq!(amount, Amount::from(42));
        assert_ne!(amount, Amount::ZERO); // Should have changed
    }

    /// Tests SubAssign actually modifies the value.
    ///
    /// Mutant testing: Catches mutation that replaces sub_assign with ().
    #[test]
    fn test_sub_assign() {
        let mut amount = Amount::from(100);
        amount -= Amount::from(30);
        assert_eq!(amount, Amount::from(70));
        assert_ne!(amount, Amount::from(100)); // Should have changed

        let mut amount = Amount::from(50);
        amount -= Amount::from(1);
        assert_eq!(amount, Amount::from(49));
        assert_ne!(amount, Amount::from(50)); // Should have changed

        let mut amount = Amount::from(10);
        amount -= Amount::from(10);
        assert_eq!(amount, Amount::ZERO);
        assert_ne!(amount, Amount::from(10)); // Should have changed
    }

    // Phase 2 tests: Amount<CurrencyUnit> methods

    #[test]
    fn test_amount_with_currency_unit() {
        let amount = Amount::new(1000, CurrencyUnit::Sat);
        assert_eq!(amount.value(), 1000);
        assert_eq!(amount.unit(), &CurrencyUnit::Sat);
    }

    #[test]
    fn test_amount_new_with_custom_unit() {
        let custom_unit = CurrencyUnit::Custom("BTC".to_string());
        let amount = Amount::new(50, custom_unit.clone());

        assert_eq!(amount.value(), 50);
        assert_eq!(amount.unit(), &custom_unit);
    }

    #[test]
    fn test_amount_into_parts() {
        let amount = Amount::new(1234, CurrencyUnit::Msat);
        let (value, unit) = amount.into_parts();

        assert_eq!(value, 1234);
        assert_eq!(unit, CurrencyUnit::Msat);
    }

    #[test]
    fn test_amount_with_unit_conversion() {
        let untyped: Amount<()> = Amount::from(100);
        let typed = untyped.with_unit(CurrencyUnit::Sat);

        assert_eq!(typed.value(), 100);
        assert_eq!(typed.unit(), &CurrencyUnit::Sat);
    }

    #[test]
    fn test_amount_with_unit_all_variants() {
        let untyped = Amount::from(500);

        let sat = untyped.with_unit(CurrencyUnit::Sat);
        assert_eq!(sat.unit(), &CurrencyUnit::Sat);

        let msat = untyped.with_unit(CurrencyUnit::Msat);
        assert_eq!(msat.unit(), &CurrencyUnit::Msat);

        let usd = untyped.with_unit(CurrencyUnit::Usd);
        assert_eq!(usd.unit(), &CurrencyUnit::Usd);

        let eur = untyped.with_unit(CurrencyUnit::Eur);
        assert_eq!(eur.unit(), &CurrencyUnit::Eur);

        let custom = untyped.with_unit(CurrencyUnit::Custom("TEST".into()));
        assert_eq!(custom.unit(), &CurrencyUnit::Custom("TEST".into()));
    }

    #[test]
    fn test_typed_amount_is_clone_not_copy() {
        let amount = Amount::new(100, CurrencyUnit::Sat);
        let cloned = amount.clone();
        // If this compiles, Clone works. Cannot test Copy directly without moving.
        assert_eq!(cloned.value(), 100);
        assert_eq!(cloned.unit(), &CurrencyUnit::Sat);
    }

    // Phase 3 tests: Protocol types verification

    #[test]
    fn test_untyped_amount_is_copy() {
        // Verify Amount<()> is Copy (required for protocol types)
        let amount: Amount<()> = Amount::from(100);
        let copy1 = amount;
        let copy2 = amount; // Should not move - verifies Copy
        assert_eq!(copy1, copy2);
    }

    #[test]
    fn test_amount_serialization_transparent() {
        // Verify Amount<()> serializes as just the number (protocol compatibility)
        let amount = Amount::from(1234);
        let json = serde_json::to_string(&amount).unwrap();
        assert_eq!(json, "1234");

        // Verify deserialization works
        let deserialized: Amount<()> = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, Amount::from(1234));
    }

    #[test]
    fn test_typed_amount_serialization() {
        // Verify Amount<CurrencyUnit> also serializes as just the number
        let amount = Amount::new(5678, CurrencyUnit::Sat);
        let json = serde_json::to_string(&amount).unwrap();
        assert_eq!(json, "5678");

        // Note: Cannot deserialize Amount<CurrencyUnit> directly
        // Unit must come from context (e.g., keyset)
    }

    #[test]
    fn test_protocol_type_pattern() {
        // Simulate protocol type usage pattern

        // Protocol layer: Amount<()> is Copy and serializes transparently
        let protocol_amount: Amount<()> = Amount::from(1000);
        let _copied = protocol_amount; // Copy works

        // Application layer: Convert to typed when needed
        let typed = protocol_amount.with_unit(CurrencyUnit::Sat);
        assert_eq!(typed.value(), 1000);

        // Back to protocol: Extract value
        let back_to_protocol = Amount::from(typed.value());
        assert_eq!(back_to_protocol, protocol_amount);
    }

    // Phase 4 tests: Unit-aware arithmetic

    #[test]
    fn test_typed_amount_checked_add() {
        let a = Amount::new(100, CurrencyUnit::Sat);
        let b = Amount::new(50, CurrencyUnit::Sat);

        let sum = a.checked_add(&b).unwrap();
        assert_eq!(sum.value(), 150);
        assert_eq!(sum.unit(), &CurrencyUnit::Sat);
    }

    #[test]
    fn test_typed_amount_add_unit_mismatch() {
        let sat = Amount::new(100, CurrencyUnit::Sat);
        let msat = Amount::new(100, CurrencyUnit::Msat);

        let result = sat.checked_add(&msat);
        assert!(result.is_err());

        match result.unwrap_err() {
            Error::UnitMismatch(u1, u2) => {
                assert_eq!(u1, CurrencyUnit::Sat);
                assert_eq!(u2, CurrencyUnit::Msat);
            }
            _ => panic!("Expected UnitMismatch error"),
        }
    }

    #[test]
    fn test_typed_amount_checked_sub() {
        let a = Amount::new(100, CurrencyUnit::Sat);
        let b = Amount::new(30, CurrencyUnit::Sat);

        let diff = a.checked_sub(&b).unwrap();
        assert_eq!(diff.value(), 70);
        assert_eq!(diff.unit(), &CurrencyUnit::Sat);
    }

    #[test]
    fn test_typed_amount_sub_unit_mismatch() {
        let sat = Amount::new(100, CurrencyUnit::Sat);
        let usd = Amount::new(30, CurrencyUnit::Usd);

        let result = sat.checked_sub(&usd);
        assert!(result.is_err());
    }

    #[test]
    fn test_typed_amount_convert_to() {
        // Sat to Msat
        let sat = Amount::new(1000, CurrencyUnit::Sat);
        let msat = sat.convert_to(&CurrencyUnit::Msat).unwrap();
        assert_eq!(msat.value(), 1_000_000);
        assert_eq!(msat.unit(), &CurrencyUnit::Msat);

        // Msat to Sat
        let msat = Amount::new(5000, CurrencyUnit::Msat);
        let sat = msat.convert_to(&CurrencyUnit::Sat).unwrap();
        assert_eq!(sat.value(), 5);
        assert_eq!(sat.unit(), &CurrencyUnit::Sat);

        // Same unit (optimization check)
        let sat = Amount::new(100, CurrencyUnit::Sat);
        let same = sat.convert_to(&CurrencyUnit::Sat).unwrap();
        assert_eq!(same.value(), 100);
        assert_eq!(same.unit(), &CurrencyUnit::Sat);
    }

    #[test]
    fn test_typed_amount_convert_invalid() {
        let sat = Amount::new(100, CurrencyUnit::Sat);
        let result = sat.convert_to(&CurrencyUnit::Eur);
        assert!(result.is_err());

        match result.unwrap_err() {
            Error::CannotConvertUnits => {}
            _ => panic!("Expected CannotConvertUnits error"),
        }
    }

    #[test]
    fn test_typed_amount_add_overflow() {
        let a = Amount::new(u64::MAX, CurrencyUnit::Sat);
        let b = Amount::new(1, CurrencyUnit::Sat);

        let result = a.checked_add(&b);
        assert!(result.is_err());

        match result.unwrap_err() {
            Error::AmountOverflow => {}
            _ => panic!("Expected AmountOverflow error"),
        }
    }

    #[test]
    fn test_typed_amount_sub_underflow() {
        let a = Amount::new(50, CurrencyUnit::Sat);
        let b = Amount::new(100, CurrencyUnit::Sat);

        let result = a.checked_sub(&b);
        assert!(result.is_err());

        match result.unwrap_err() {
            Error::AmountOverflow => {} // Underflow also returns AmountOverflow
            _ => panic!("Expected AmountOverflow error"),
        }
    }

    // Phase 5 tests: PartialOrd behavior for Amount<CurrencyUnit>

    /// Tests that equality works correctly for typed amounts with the same unit.
    #[test]
    fn test_typed_amount_equality_same_unit() {
        let a = Amount::new(100, CurrencyUnit::Sat);
        let b = Amount::new(100, CurrencyUnit::Sat);

        assert_eq!(a, b);
        assert!(a == b);

        let c = Amount::new(50, CurrencyUnit::Sat);
        assert_ne!(a, c);
        assert!(a != c);
    }

    /// Tests that equality returns false for typed amounts with different units.
    #[test]
    fn test_typed_amount_equality_different_units() {
        let sat = Amount::new(100, CurrencyUnit::Sat);
        let msat = Amount::new(100, CurrencyUnit::Msat);

        // Same value, different units - should NOT be equal
        assert_ne!(sat, msat);
        assert!(sat != msat);

        let usd = Amount::new(100, CurrencyUnit::Usd);
        assert_ne!(sat, usd);
        assert_ne!(msat, usd);
    }

    /// Tests that comparison operators work correctly for typed amounts with the same unit.
    #[test]
    fn test_typed_amount_comparison_same_unit() {
        let small = Amount::new(50, CurrencyUnit::Sat);
        let large = Amount::new(100, CurrencyUnit::Sat);

        // Greater than
        assert!(large > small);
        assert!(!(small > large));

        // Less than
        assert!(small < large);
        assert!(!(large < small));

        // Greater than or equal
        assert!(large >= small);
        assert!(large >= Amount::new(100, CurrencyUnit::Sat));

        // Less than or equal
        assert!(small <= large);
        assert!(small <= Amount::new(50, CurrencyUnit::Sat));

        // partial_cmp returns Some
        assert_eq!(large.partial_cmp(&small), Some(std::cmp::Ordering::Greater));
        assert_eq!(small.partial_cmp(&large), Some(std::cmp::Ordering::Less));
        assert_eq!(
            small.partial_cmp(&Amount::new(50, CurrencyUnit::Sat)),
            Some(std::cmp::Ordering::Equal)
        );
    }

    /// Tests that partial_cmp returns None for typed amounts with different units.
    /// This ensures that comparisons between different units are not accidentally valid.
    #[test]
    fn test_typed_amount_comparison_different_units_returns_none() {
        let sat = Amount::new(100, CurrencyUnit::Sat);
        let msat = Amount::new(50, CurrencyUnit::Msat);

        // partial_cmp should return None for different units
        assert_eq!(sat.partial_cmp(&msat), None);
        assert_eq!(msat.partial_cmp(&sat), None);

        // Different unit combinations
        let usd = Amount::new(100, CurrencyUnit::Usd);
        assert_eq!(sat.partial_cmp(&usd), None);
        assert_eq!(usd.partial_cmp(&sat), None);

        let eur = Amount::new(100, CurrencyUnit::Eur);
        assert_eq!(usd.partial_cmp(&eur), None);

        let custom = Amount::new(100, CurrencyUnit::Custom("BTC".into()));
        assert_eq!(sat.partial_cmp(&custom), None);
    }

    /// Tests that comparison operators return false when comparing different units.
    /// Since partial_cmp returns None, all comparisons should be false.
    #[test]
    fn test_typed_amount_comparison_operators_different_units() {
        let sat = Amount::new(100, CurrencyUnit::Sat);
        let msat = Amount::new(50, CurrencyUnit::Msat);

        // When partial_cmp returns None:
        // - > returns false
        // - < returns false
        // - >= returns false
        // - <= returns false

        assert!(!(sat > msat));
        assert!(!(sat < msat));
        assert!(!(sat >= msat));
        assert!(!(sat <= msat));

        assert!(!(msat > sat));
        assert!(!(msat < sat));
        assert!(!(msat >= sat));
        assert!(!(msat <= sat));

        // Even with same value, different units should return false
        let sat100 = Amount::new(100, CurrencyUnit::Sat);
        let msat100 = Amount::new(100, CurrencyUnit::Msat);

        assert!(!(sat100 > msat100));
        assert!(!(sat100 < msat100));
        assert!(!(sat100 >= msat100));
        assert!(!(sat100 <= msat100));
    }

    /// Tests that Amount<()> (untyped) has total ordering and implements Ord.
    #[test]
    fn test_untyped_amount_has_total_ordering() {
        use std::cmp::Ordering;

        let a: Amount<()> = Amount::from(50);
        let b: Amount<()> = Amount::from(100);
        let c: Amount<()> = Amount::from(50);

        // Ord::cmp is available for Amount<()>
        assert_eq!(a.cmp(&b), Ordering::Less);
        assert_eq!(b.cmp(&a), Ordering::Greater);
        assert_eq!(a.cmp(&c), Ordering::Equal);

        // PartialOrd returns Some (total ordering)
        assert_eq!(a.partial_cmp(&b), Some(Ordering::Less));
        assert_eq!(b.partial_cmp(&a), Some(Ordering::Greater));
        assert_eq!(a.partial_cmp(&c), Some(Ordering::Equal));
    }

    /// Tests that Amount<()> can be sorted (requires Ord).
    #[test]
    fn test_untyped_amount_sorting() {
        let mut amounts: Vec<Amount<()>> = vec![
            Amount::from(100),
            Amount::from(25),
            Amount::from(75),
            Amount::from(50),
        ];

        amounts.sort();

        assert_eq!(
            amounts,
            vec![
                Amount::from(25),
                Amount::from(50),
                Amount::from(75),
                Amount::from(100),
            ]
        );
    }
}

//! Amount and currency related types

use cdk::nuts::CurrencyUnit as CdkCurrencyUnit;
use cdk::Amount as CdkAmount;
use serde::{Deserialize, Serialize};

use crate::error::FfiError;

/// FFI-compatible Amount type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
#[serde(transparent)]
pub struct Amount {
    pub value: u64,
}

impl Amount {
    pub fn new(value: u64) -> Self {
        Self { value }
    }

    pub fn zero() -> Self {
        Self { value: 0 }
    }

    pub fn is_zero(&self) -> bool {
        self.value == 0
    }

    pub fn convert_unit(
        &self,
        current_unit: CurrencyUnit,
        target_unit: CurrencyUnit,
    ) -> Result<Amount, FfiError> {
        Ok(CdkAmount::from(self.value)
            .convert_unit(&current_unit.into(), &target_unit.into())
            .map(Into::into)?)
    }

    pub fn add(&self, other: Amount) -> Result<Amount, FfiError> {
        let self_amount = CdkAmount::from(self.value);
        let other_amount = CdkAmount::from(other.value);
        self_amount
            .checked_add(other_amount)
            .map(Into::into)
            .ok_or(FfiError::AmountOverflow)
    }

    pub fn subtract(&self, other: Amount) -> Result<Amount, FfiError> {
        let self_amount = CdkAmount::from(self.value);
        let other_amount = CdkAmount::from(other.value);
        self_amount
            .checked_sub(other_amount)
            .map(Into::into)
            .ok_or(FfiError::AmountOverflow)
    }

    pub fn multiply(&self, factor: u64) -> Result<Amount, FfiError> {
        let self_amount = CdkAmount::from(self.value);
        let factor_amount = CdkAmount::from(factor);
        self_amount
            .checked_mul(factor_amount)
            .map(Into::into)
            .ok_or(FfiError::AmountOverflow)
    }

    pub fn divide(&self, divisor: u64) -> Result<Amount, FfiError> {
        if divisor == 0 {
            return Err(FfiError::DivisionByZero);
        }
        let self_amount = CdkAmount::from(self.value);
        let divisor_amount = CdkAmount::from(divisor);
        self_amount
            .checked_div(divisor_amount)
            .map(Into::into)
            .ok_or(FfiError::AmountOverflow)
    }
}

impl From<CdkAmount> for Amount {
    fn from(amount: CdkAmount) -> Self {
        Self {
            value: u64::from(amount),
        }
    }
}

impl From<Amount> for CdkAmount {
    fn from(amount: Amount) -> Self {
        CdkAmount::from(amount.value)
    }
}

/// FFI-compatible Currency Unit
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum CurrencyUnit {
    Sat,
    Msat,
    Usd,
    Eur,
    Auth,
    Custom { unit: String },
}

impl From<CdkCurrencyUnit> for CurrencyUnit {
    fn from(unit: CdkCurrencyUnit) -> Self {
        match unit {
            CdkCurrencyUnit::Sat => CurrencyUnit::Sat,
            CdkCurrencyUnit::Msat => CurrencyUnit::Msat,
            CdkCurrencyUnit::Usd => CurrencyUnit::Usd,
            CdkCurrencyUnit::Eur => CurrencyUnit::Eur,
            CdkCurrencyUnit::Auth => CurrencyUnit::Auth,
            CdkCurrencyUnit::Custom(s) => CurrencyUnit::Custom { unit: s },
            _ => CurrencyUnit::Sat, // Default for unknown units
        }
    }
}

impl From<CurrencyUnit> for CdkCurrencyUnit {
    fn from(unit: CurrencyUnit) -> Self {
        match unit {
            CurrencyUnit::Sat => CdkCurrencyUnit::Sat,
            CurrencyUnit::Msat => CdkCurrencyUnit::Msat,
            CurrencyUnit::Usd => CdkCurrencyUnit::Usd,
            CurrencyUnit::Eur => CdkCurrencyUnit::Eur,
            CurrencyUnit::Auth => CdkCurrencyUnit::Auth,
            CurrencyUnit::Custom { unit } => CdkCurrencyUnit::custom(unit),
        }
    }
}

/// FFI-compatible SplitTarget
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Enum)]
pub enum SplitTarget {
    /// Default target; least amount of proofs
    None,
    /// Target amount for wallet to have most proofs that add up to value
    Value { amount: Amount },
    /// Specific amounts to split into (must equal amount being split)
    Values { amounts: Vec<Amount> },
}

impl From<SplitTarget> for cdk::amount::SplitTarget {
    fn from(target: SplitTarget) -> Self {
        match target {
            SplitTarget::None => cdk::amount::SplitTarget::None,
            SplitTarget::Value { amount } => cdk::amount::SplitTarget::Value(amount.into()),
            SplitTarget::Values { amounts } => {
                cdk::amount::SplitTarget::Values(amounts.into_iter().map(Into::into).collect())
            }
        }
    }
}

impl From<cdk::amount::SplitTarget> for SplitTarget {
    fn from(target: cdk::amount::SplitTarget) -> Self {
        match target {
            cdk::amount::SplitTarget::None => SplitTarget::None,
            cdk::amount::SplitTarget::Value(amount) => SplitTarget::Value {
                amount: amount.into(),
            },
            cdk::amount::SplitTarget::Values(amounts) => SplitTarget::Values {
                amounts: amounts.into_iter().map(Into::into).collect(),
            },
        }
    }
}

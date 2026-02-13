//! Amount and currency related types

use cdk::nuts::CurrencyUnit as CdkCurrencyUnit;
use cdk::Amount as CdkAmount;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

/// WASM-compatible Amount type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
#[wasm_bindgen]
pub struct Amount {
    #[wasm_bindgen(readonly)]
    pub value: u64,
}

#[wasm_bindgen]
impl Amount {
    #[wasm_bindgen(constructor)]
    pub fn new(value: u64) -> Self {
        Self { value }
    }

    pub fn zero() -> Self {
        Self { value: 0 }
    }

    #[wasm_bindgen(js_name = "isZero")]
    pub fn is_zero(&self) -> bool {
        self.value == 0
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

/// WASM-compatible Currency Unit
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
            _ => CurrencyUnit::Sat,
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
            CurrencyUnit::Custom { unit } => CdkCurrencyUnit::Custom(unit),
        }
    }
}

/// WASM-compatible SplitTarget
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SplitTarget {
    None,
    Value { amount: Amount },
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

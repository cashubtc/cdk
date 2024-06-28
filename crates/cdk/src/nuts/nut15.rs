//! NUT-15: Multipart payments
//!
//! <https://github.com/cashubtc/nuts/blob/main/15.md>

use serde::{Deserialize, Serialize};

use super::{CurrencyUnit, PaymentMethod};
use crate::Amount;

/// Multi-part payment
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename = "lowercase")]
pub struct Mpp {
    /// Amount
    pub amount: Amount,
}

/// Mpp Method Settings
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MppMethodSettings {
    /// Payment Method e.g. bolt11
    pub method: PaymentMethod,
    /// Currency Unit e.g. sat
    pub unit: CurrencyUnit,
    /// Multi part payment support
    pub mpp: bool,
}

/// Mpp Settings
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Settings {
    /// Method settings
    pub methods: Vec<MppMethodSettings>,
}

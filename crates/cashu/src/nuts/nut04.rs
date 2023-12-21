//! Mint Tokens
// https://github.com/cashubtc/nuts/blob/main/04.md
use serde::{Deserialize, Serialize};

use super::{BlindedMessage, BlindedSignature, CurrencyUnit, PaymentMethod};
use crate::Amount;

/// Mint quote request [NUT-04]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintQuoteBolt11Request {
    /// Amount
    pub amount: Amount,
    /// Unit wallet would like to pay with
    pub unit: CurrencyUnit,
}

/// Mint quote response [NUT-04]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintQuoteBolt11Response {
    /// Quote Id
    pub quote: String,
    /// Payment request to fulfil
    pub request: String,
    /// Whether the the request haas be paid
    pub paid: bool,
    /// Unix timestamp until the quote is valid
    pub expiry: u64,
}

/// Mint request [NUT-04]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintBolt11Request {
    /// Quote id
    pub quote: String,
    /// Outputs
    pub outputs: Vec<BlindedMessage>,
}

impl MintBolt11Request {
    pub fn total_amount(&self) -> Amount {
        self.outputs
            .iter()
            .map(|BlindedMessage { amount, .. }| *amount)
            .sum()
    }
}

/// Mint response [NUT-04]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintBolt11Response {
    /// Blinded Signatures
    pub signatures: Vec<BlindedSignature>,
}

/// Mint Settings
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    methods: Vec<(PaymentMethod, CurrencyUnit)>,
    disabled: bool,
}

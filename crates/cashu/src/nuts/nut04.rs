//! Mint Tokens via Bolt11
// https://github.com/cashubtc/nuts/blob/main/04.md
use serde::{Deserialize, Serialize};

use super::{BlindedMessage, BlindedSignature, CurrencyUnit, PaymentMethod};
use crate::types::MintQuote;
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

impl From<MintQuote> for MintQuoteBolt11Response {
    fn from(mint_quote: MintQuote) -> MintQuoteBolt11Response {
        MintQuoteBolt11Response {
            quote: mint_quote.id,
            request: mint_quote.request,
            paid: mint_quote.paid,
            expiry: mint_quote.expiry,
        }
    }
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

/// Mint Method Settings
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintMethodSettings {
    /// Payment Method e.g. bolt11
    method: PaymentMethod,
    /// Currency Unit e.g. sat
    unit: CurrencyUnit,
    /// Min Amount
    min_amount: Amount,
    /// Max Amount
    max_amount: Amount,
}

/// Mint Settings
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    methods: Vec<MintMethodSettings>,
    disabled: bool,
}

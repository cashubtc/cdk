//! NUT-05: Melting Tokens
//!
//! <https://github.com/cashubtc/nuts/blob/main/05.md>

use serde::{Deserialize, Serialize};

use super::nut00::{BlindSignature, BlindedMessage, CurrencyUnit, PaymentMethod, Proofs};
use crate::types::MeltQuote;
use crate::{Amount, Bolt11Invoice};

/// Melt quote request [NUT-05]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltQuoteBolt11Request {
    /// Bolt11 invoice to be paid
    pub request: Bolt11Invoice,
    /// Unit wallet would like to pay with
    pub unit: CurrencyUnit,
}

/// Melt quote response [NUT-05]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltQuoteBolt11Response {
    /// Quote Id
    pub quote: String,
    /// The amount that needs to be provided
    pub amount: Amount,
    /// The fee reserve that is required
    pub fee_reserve: Amount,
    /// Whether the the request haas be paid
    pub paid: bool,
    /// Unix timestamp until the quote is valid
    pub expiry: u64,
}

impl From<MeltQuote> for MeltQuoteBolt11Response {
    fn from(melt_quote: MeltQuote) -> MeltQuoteBolt11Response {
        MeltQuoteBolt11Response {
            quote: melt_quote.id,
            amount: melt_quote.amount,
            fee_reserve: melt_quote.fee_reserve,
            paid: melt_quote.paid,
            expiry: melt_quote.expiry,
        }
    }
}

/// Melt Bolt11 Request [NUT-05]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltBolt11Request {
    /// Quote ID
    pub quote: String,
    /// Proofs
    pub inputs: Proofs,
    /// Blinded Message that can be used to return change [NUT-08]
    /// Amount field of BlindedMessages `SHOULD` be set to zero
    pub outputs: Option<Vec<BlindedMessage>>,
}

impl MeltBolt11Request {
    pub fn proofs_amount(&self) -> Amount {
        self.inputs.iter().map(|proof| proof.amount).sum()
    }
}

/// Melt Response [NUT-05]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltBolt11Response {
    /// Indicate if payment was successful
    pub paid: bool,
    /// Bolt11 preimage
    pub payment_preimage: Option<String>,
    /// Change
    pub change: Option<Vec<BlindSignature>>,
}

/// Melt Method Settings
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MeltMethodSettings {
    /// Payment Method e.g. bolt11
    method: PaymentMethod,
    /// Currency Unit e.g. sat
    unit: CurrencyUnit,
    /// Min Amount
    #[serde(skip_serializing_if = "Option::is_none")]
    min_amount: Option<Amount>,
    /// Max Amount
    #[serde(skip_serializing_if = "Option::is_none")]
    max_amount: Option<Amount>,
}

/// Melt Settings
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Settings {
    methods: Vec<MeltMethodSettings>,
    disabled: bool,
}

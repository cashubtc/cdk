//! Melting Tokens
// https://github.com/cashubtc/nuts/blob/main/05.md

use serde::{Deserialize, Serialize};

use super::CurrencyUnit;
use crate::nuts::Proofs;
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
    pub amount: u64,
    /// The fee reserve that is required
    pub fee_reserve: u64,
    /// Whether the the request haas be paid
    pub paid: bool,
    /// Unix timestamp until the quote is valid
    pub expiry: u64,
}

/// Melt Bolt11 Request [NUT-05]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltBolt11Request {
    /// Quote ID
    pub quote: String,
    /// Proofs
    pub inputs: Proofs,
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
    pub payment_preimage: String,
}

/// Melt Settings
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Settings {
    methods: Vec<(String, CurrencyUnit)>,
}

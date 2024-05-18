//! Types

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::nuts::{CurrencyUnit, Proofs};
use crate::url::UncheckedUrl;
use crate::Amount;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofsStatus {
    pub spendable: Proofs,
    pub spent: Proofs,
}

/// Melt response with proofs
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Melted {
    pub paid: bool,
    pub preimage: Option<String>,
    pub change: Option<Proofs>,
}

/// Possible states of an invoice
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq)]
pub enum InvoiceStatus {
    Unpaid,
    Paid,
    Expired,
    InFlight,
}

/// Mint Quote Info
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct MintQuote {
    pub id: String,
    pub mint_url: UncheckedUrl,
    pub amount: Amount,
    pub unit: CurrencyUnit,
    pub request: String,
    pub paid: bool,
    pub expiry: u64,
}

impl MintQuote {
    pub fn new(
        mint_url: UncheckedUrl,
        request: String,
        unit: CurrencyUnit,
        amount: Amount,
        expiry: u64,
    ) -> Self {
        let id = Uuid::new_v4();

        Self {
            mint_url,
            id: id.to_string(),
            amount,
            unit,
            request,
            paid: false,
            expiry,
        }
    }
}

/// Melt Quote Info
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct MeltQuote {
    pub id: String,
    pub unit: CurrencyUnit,
    pub amount: Amount,
    pub request: String,
    pub fee_reserve: Amount,
    pub paid: bool,
    pub expiry: u64,
}

impl MeltQuote {
    pub fn new(
        request: String,
        unit: CurrencyUnit,
        amount: Amount,
        fee_reserve: Amount,
        expiry: u64,
    ) -> Self {
        let id = Uuid::new_v4();

        Self {
            id: id.to_string(),
            amount,
            unit,
            request,
            fee_reserve,
            paid: false,
            expiry,
        }
    }
}

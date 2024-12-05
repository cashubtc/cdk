//! Mint Types

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::CurrencyUnit;
use crate::mint_url::MintUrl;
use crate::nuts::{MeltQuoteState, MintQuoteState};
use crate::Amount;

/// Mint Quote Info
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintQuote {
    /// Quote id
    pub id: Uuid,
    /// Mint Url
    pub mint_url: MintUrl,
    /// Amount of quote
    pub amount: Amount,
    /// Unit of quote
    pub unit: CurrencyUnit,
    /// Quote payment request e.g. bolt11
    pub request: String,
    /// Quote state
    pub state: MintQuoteState,
    /// Expiration time of quote
    pub expiry: u64,
    /// Value used by ln backend to look up state of request
    pub request_lookup_id: String,
}

impl MintQuote {
    /// Create new [`MintQuote`]
    pub fn new(
        mint_url: MintUrl,
        request: String,
        unit: CurrencyUnit,
        amount: Amount,
        expiry: u64,
        request_lookup_id: String,
    ) -> Self {
        let id = Uuid::new_v4();

        Self {
            mint_url,
            id,
            amount,
            unit,
            request,
            state: MintQuoteState::Unpaid,
            expiry,
            request_lookup_id,
        }
    }
}

/// Melt Quote Info
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltQuote {
    /// Quote id
    pub id: Uuid,
    /// Quote unit
    pub unit: CurrencyUnit,
    /// Quote amount
    pub amount: Amount,
    /// Quote Payment request e.g. bolt11
    pub request: String,
    /// Quote fee reserve
    pub fee_reserve: Amount,
    /// Quote state
    pub state: MeltQuoteState,
    /// Expiration time of quote
    pub expiry: u64,
    /// Payment preimage
    pub payment_preimage: Option<String>,
    /// Value used by ln backend to look up state of request
    pub request_lookup_id: String,
}

impl MeltQuote {
    /// Create new [`MeltQuote`]
    pub fn new(
        request: String,
        unit: CurrencyUnit,
        amount: Amount,
        fee_reserve: Amount,
        expiry: u64,
        request_lookup_id: String,
    ) -> Self {
        let id = Uuid::new_v4();

        Self {
            id,
            amount,
            unit,
            request,
            fee_reserve,
            state: MeltQuoteState::Unpaid,
            expiry,
            payment_preimage: None,
            request_lookup_id,
        }
    }
}

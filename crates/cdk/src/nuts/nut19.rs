//! NUT-17: Mint Tokens via Bolt11
//!
//! <https://github.com/cashubtc/nuts/blob/main/04.md>

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::nut00::CurrencyUnit;
use crate::Amount;

/// NUT04 Error
#[derive(Debug, Error)]
pub enum Error {
    /// Unknown Quote State
    #[error("Unknown Quote State")]
    UnknownState,
    /// Amount overflow
    #[error("Amount overflow")]
    AmountOverflow,
}

/// Mint quote request [NUT-19]
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintQuoteBolt12Request {
    /// Amount
    pub amount: Option<Amount>,
    /// Unit wallet would like to pay with
    pub unit: CurrencyUnit,
    /// Memo to create the invoice with
    pub description: Option<String>,
    /// Single use
    pub single_use: Option<bool>,
    /// Expiry
    pub expiry: Option<u64>,
}

/// Mint quote response [NUT-19]
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintQuoteBolt12Response {
    /// Quote Id
    pub quote: String,
    /// Payment request to fulfil
    pub request: String,
    /// Single use
    pub single_use: Option<bool>,
    /// Unix timestamp until the quote is valid
    pub expiry: Option<u64>,
    /// Amount that has been paid
    pub amount_paid: Amount,
    /// Amount that has been issued
    pub amount_issued: Amount,
}

#[cfg(feature = "mint")]
impl From<crate::mint::MintQuote> for MintQuoteBolt12Response {
    fn from(mint_quote: crate::mint::MintQuote) -> MintQuoteBolt12Response {
        MintQuoteBolt12Response {
            quote: mint_quote.id,
            request: mint_quote.request,
            expiry: Some(mint_quote.expiry),
            amount_paid: mint_quote.amount_paid,
            amount_issued: mint_quote.amount_issued,
            single_use: mint_quote.single_use,
        }
    }
}

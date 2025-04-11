//! NUT-23: Mint Tokens via Bolt12
//!
//! <https://github.com/cashubtc/nuts/blob/main/23.md>

use serde::{Deserialize, Serialize};
use thiserror::Error;
#[cfg(feature = "mint")]
use uuid::Uuid;

use super::nut00::CurrencyUnit;
use super::PublicKey;
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
    /// Publickey not defined
    #[error("Publickey not defined")]
    PublickeyUndefined,
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
    pub single_use: bool,
    /// Expiry
    pub expiry: Option<u64>,
    /// Pubkey
    pub pubkey: PublicKey,
}

/// Mint quote response [NUT-19]
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound = "Q: Serialize + for<'a> Deserialize<'a>")]
pub struct MintQuoteBolt12Response<Q> {
    /// Quote Id
    pub quote: Q,
    /// Payment request to fulfil
    pub request: String,
    /// Single use
    pub single_use: bool,
    /// Unix timestamp until the quote is valid
    pub expiry: Option<u64>,
    /// Amount that has been paid
    pub amount_paid: Amount,
    /// Amount that has been issued
    pub amount_issued: Amount,
    /// Pubkey
    pub pubkey: PublicKey,
}

#[cfg(feature = "mint")]
impl From<MintQuoteBolt12Response<Uuid>> for MintQuoteBolt12Response<String> {
    fn from(value: MintQuoteBolt12Response<Uuid>) -> Self {
        Self {
            quote: value.quote.to_string(),
            request: value.request,
            single_use: value.single_use,
            expiry: value.expiry,
            amount_paid: value.amount_paid,
            amount_issued: value.amount_issued,
            pubkey: value.pubkey,
        }
    }
}

//! Bolt12
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{CurrencyUnit, MeltOptions, PublicKey};
#[cfg(feature = "mint")]
use crate::quote_id::QuoteId;
use crate::Amount;

/// NUT18 Error
#[derive(Debug, Error)]
pub enum Error {
    /// Unknown Quote State
    #[error("Unknown quote state")]
    UnknownState,
    /// Amount overflow
    #[error("Amount Overflow")]
    AmountOverflow,
    /// Publickey not defined
    #[error("Publickey not defined")]
    PublickeyUndefined,
}

/// Mint quote request [NUT-24]
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintQuoteBolt12Request {
    /// Amount
    pub amount: Option<Amount>,
    /// Unit wallet would like to pay with
    pub unit: CurrencyUnit,
    /// Memo to create the invoice with
    pub description: Option<String>,
    /// Pubkey
    pub pubkey: PublicKey,
}

/// Mint quote response [NUT-24]
///
/// `deny_unknown_fields` is intentional: the `NotificationPayload` enum is
/// `#[serde(untagged)]` and Bolt11 mint quotes share the same core fields as
/// Bolt12 mint quotes. Rejecting unknown fields lets `NotificationPayload`
/// try the Bolt12 variant before Bolt11 without classifying Bolt11 payloads
/// that carry a `state` field as Bolt12.
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound = "Q: Serialize + for<'a> Deserialize<'a>")]
#[serde(deny_unknown_fields)]
pub struct MintQuoteBolt12Response<Q> {
    /// Quote Id
    pub quote: Q,
    /// Payment request to fulfil
    pub request: String,
    /// Amount
    pub amount: Option<Amount>,
    /// Unit wallet would like to pay with
    pub unit: CurrencyUnit,
    /// Unix timestamp until the quote is valid
    pub expiry: Option<u64>,
    /// Pubkey
    pub pubkey: PublicKey,
    /// Amount that has been paid
    pub amount_paid: Amount,
    /// Amount that has been issued
    pub amount_issued: Amount,
    /// Unix timestamp indicating when the quote was last updated
    #[serde(default)]
    pub updated_at: u64,
}

#[cfg(feature = "mint")]
impl<Q: ToString> MintQuoteBolt12Response<Q> {
    /// Convert the MintQuote with a quote type Q to a String
    pub fn to_string_id(&self) -> MintQuoteBolt12Response<String> {
        MintQuoteBolt12Response {
            quote: self.quote.to_string(),
            request: self.request.clone(),
            amount: self.amount,
            unit: self.unit.clone(),
            expiry: self.expiry,
            pubkey: self.pubkey,
            amount_paid: self.amount_paid,
            amount_issued: self.amount_issued,
            updated_at: self.updated_at,
        }
    }
}

#[cfg(feature = "mint")]
impl From<MintQuoteBolt12Response<QuoteId>> for MintQuoteBolt12Response<String> {
    fn from(value: MintQuoteBolt12Response<QuoteId>) -> Self {
        Self {
            quote: value.quote.to_string(),
            request: value.request,
            expiry: value.expiry,
            amount_paid: value.amount_paid,
            amount_issued: value.amount_issued,
            pubkey: value.pubkey,
            amount: value.amount,
            unit: value.unit,
            updated_at: value.updated_at,
        }
    }
}

/// Melt quote request [NUT-18]
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltQuoteBolt12Request {
    /// Bolt12 invoice to be paid
    pub request: String,
    /// Unit wallet would like to pay with
    pub unit: CurrencyUnit,
    /// Payment Options
    pub options: Option<MeltOptions>,
}

/// Melt quote response [NUT-25]
pub type MeltQuoteBolt12Response<Q> = crate::nuts::nut23::MeltQuoteBolt11Response<Q>;

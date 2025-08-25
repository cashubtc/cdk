//! Onchain
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;
#[cfg(feature = "mint")]
use uuid::Uuid;

use super::{BlindSignature, CurrencyUnit, MeltQuoteState, PublicKey};
use crate::Amount;

/// NUT-26 Error
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

/// Mint quote request [NUT-26]
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct MintQuoteOnchainRequest {
    /// Unit wallet would like to pay with
    pub unit: CurrencyUnit,
    /// Pubkey
    pub pubkey: PublicKey,
}

/// Mint quote response [NUT-26]
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
#[serde(bound = "Q: Serialize + DeserializeOwned")]
pub struct MintQuoteOnchainResponse<Q> {
    /// Quote Id
    pub quote: Q,
    /// Payment request to fulfil
    pub request: String,
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
    /// Amount that is waiting to be deep enough in the chain to be confirmed
    pub amount_unconfirmed: Amount,
}

#[cfg(feature = "mint")]
impl<Q: ToString> MintQuoteOnchainResponse<Q> {
    /// Convert the MintQuote with a quote type Q to a String
    pub fn to_string_id(&self) -> MintQuoteOnchainResponse<String> {
        MintQuoteOnchainResponse {
            quote: self.quote.to_string(),
            request: self.request.clone(),
            unit: self.unit.clone(),
            expiry: self.expiry,
            pubkey: self.pubkey,
            amount_paid: self.amount_paid,
            amount_issued: self.amount_issued,
            amount_unconfirmed: self.amount_unconfirmed,
        }
    }
}

#[cfg(feature = "mint")]
impl From<MintQuoteOnchainResponse<Uuid>> for MintQuoteOnchainResponse<String> {
    fn from(value: MintQuoteOnchainResponse<Uuid>) -> Self {
        Self {
            quote: value.quote.to_string(),
            request: value.request,
            unit: value.unit,
            expiry: value.expiry,
            pubkey: value.pubkey,
            amount_paid: value.amount_paid,
            amount_issued: value.amount_issued,
            amount_unconfirmed: value.amount_unconfirmed,
        }
    }
}

/// Melt quote request [NUT-26]
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct MeltQuoteOnchainRequest {
    /// Onchain address to send to
    pub request: String,
    /// Unit wallet would like to pay with
    pub unit: CurrencyUnit,
    /// Amount to pay
    pub amount: Amount,
}

/// Melt quote response [NUT-26]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
#[serde(bound = "Q: Serialize + DeserializeOwned")]
pub struct MeltQuoteOnchainResponse<Q> {
    /// Quote Id
    pub quote: Q,
    /// The amount that needs to be provided
    pub amount: Amount,
    /// Payment request to fulfill
    pub request: String,
    /// Unit
    pub unit: CurrencyUnit,
    /// The fee reserve that is required
    pub fee_reserve: Amount,
    /// Quote State
    pub state: MeltQuoteState,
    /// Unix timestamp until the quote is valid
    // TODO: is this needed?
    pub expiry: u64,
    /// Transaction ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction_id: Option<String>,
    /// Change
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<Vec<BlindSignature>>,
}

impl<Q: ToString> MeltQuoteOnchainResponse<Q> {
    /// Convert a `MeltQuoteOnchainResponse` with type Q (generic/unknown) to a
    /// `MeltQuoteOnchainResponse` with `String`
    pub fn to_string_id(self) -> MeltQuoteOnchainResponse<String> {
        MeltQuoteOnchainResponse {
            quote: self.quote.to_string(),
            amount: self.amount,
            fee_reserve: self.fee_reserve,
            state: self.state,
            expiry: self.expiry,
            transaction_id: self.transaction_id,
            change: self.change,
            request: self.request,
            unit: self.unit,
        }
    }
}

#[cfg(feature = "mint")]
impl From<MeltQuoteOnchainResponse<Uuid>> for MeltQuoteOnchainResponse<String> {
    fn from(value: MeltQuoteOnchainResponse<Uuid>) -> Self {
        Self {
            quote: value.quote.to_string(),
            amount: value.amount,
            fee_reserve: value.fee_reserve,
            state: value.state,
            expiry: value.expiry,
            transaction_id: value.transaction_id,
            change: value.change,
            request: value.request,
            unit: value.unit,
        }
    }
}

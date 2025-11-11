//! Custom Payment Methods
//!
//! This module defines generic request and response types for custom payment methods.
//! Unlike Bolt11/Bolt12, custom payment methods use opaque JSON data that is passed
//! directly to the payment processor without validation at the mint layer.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{CurrencyUnit, PublicKey};
use crate::nut23::QuoteState;
#[cfg(feature = "mint")]
use crate::quote_id::QuoteId;
use crate::Amount;

/// Custom payment method mint quote request
///
/// This is a generic request type that works for any custom payment method.
/// The `data` field contains method-specific information as opaque JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct MintQuoteCustomRequest {
    /// Amount to mint
    pub amount: Amount,
    /// Currency unit
    pub unit: CurrencyUnit,
    /// Method-specific data (opaque JSON)
    #[serde(default)]
    pub data: Value,
    /// Optional description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// NUT-19 Pubkey
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pubkey: Option<PublicKey>,
}

/// Custom payment method mint quote response
///
/// This is a generic response type for custom payment methods.
/// The `data` field contains method-specific response data as opaque JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
#[serde(bound = "Q: Serialize + for<'a> Deserialize<'a>")]
pub struct MintQuoteCustomResponse<Q> {
    /// Quote ID
    pub quote: Q,
    /// Payment request string (method-specific format)
    pub request: String,
    /// Amount
    pub amount: Option<Amount>,
    /// Currency unit
    pub unit: Option<CurrencyUnit>,
    /// Quote State
    pub state: QuoteState,
    /// Unix timestamp until the quote is valid
    pub expiry: Option<u64>,
    /// NUT-19 Pubkey
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pubkey: Option<PublicKey>,
    /// Method-specific response data (opaque JSON)
    #[serde(default)]
    pub data: Value,
}

#[cfg(feature = "mint")]
impl<Q: ToString> MintQuoteCustomResponse<Q> {
    /// Convert the MintQuoteCustomResponse with a quote type Q to a String
    pub fn to_string_id(&self) -> MintQuoteCustomResponse<String> {
        MintQuoteCustomResponse {
            quote: self.quote.to_string(),
            request: self.request.clone(),
            amount: self.amount,
            state: self.state,
            unit: self.unit.clone(),
            expiry: self.expiry,
            pubkey: self.pubkey,
            data: self.data.clone(),
        }
    }
}

#[cfg(feature = "mint")]
impl From<MintQuoteCustomResponse<QuoteId>> for MintQuoteCustomResponse<String> {
    fn from(value: MintQuoteCustomResponse<QuoteId>) -> Self {
        Self {
            quote: value.quote.to_string(),
            request: value.request,
            amount: value.amount,
            unit: value.unit,
            expiry: value.expiry,
            state: value.state,
            pubkey: value.pubkey,
            data: value.data,
        }
    }
}

/// Custom payment method melt quote request
///
/// This is a generic request type for melting tokens with custom payment methods.
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct MeltQuoteCustomRequest {
    /// Custom payment method name
    pub method: String,
    /// Payment request string (method-specific format)
    pub request: String,
    /// Currency unit
    pub unit: CurrencyUnit,
    /// Method-specific data (opaque JSON)
    #[serde(default)]
    pub data: Value,
}

// Note: For custom payment method melt quote responses, we reuse the standard
// MeltQuoteBolt11Response structure since the response format is already generic enough.
// The payment processor returns method-specific data in the response.

//! NUT-04: Mint Tokens via Bolt11
//!
//! <https://github.com/cashubtc/nuts/blob/main/04.md>

use std::fmt;
use std::str::FromStr;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;
#[cfg(feature = "mint")]
use uuid::Uuid;

use super::nut00::{BlindSignature, BlindedMessage, CurrencyUnit, PaymentMethod};
use super::{MintQuoteState, PublicKey};
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

/// Mint quote request [NUT-04]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct MintQuoteBolt11Request {
    /// Amount
    pub amount: Amount,
    /// Unit wallet would like to pay with
    pub unit: CurrencyUnit,
    /// Memo to create the invoice with
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// NUT-19 Pubkey
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pubkey: Option<PublicKey>,
}

/// Possible states of a quote
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema), schema(as = MintQuoteState))]
pub enum QuoteState {
    /// Quote has not been paid
    #[default]
    Unpaid,
    /// Quote has been paid and wallet can mint
    Paid,
    /// Minting is in progress
    /// **Note:** This state is to be used internally but is not part of the
    /// nut.
    Pending,
    /// ecash issued for quote
    Issued,
}

impl fmt::Display for QuoteState {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Unpaid => write!(f, "UNPAID"),
            Self::Paid => write!(f, "PAID"),
            Self::Pending => write!(f, "PENDING"),
            Self::Issued => write!(f, "ISSUED"),
        }
    }
}

impl FromStr for QuoteState {
    type Err = Error;

    fn from_str(state: &str) -> Result<Self, Self::Err> {
        match state {
            "PENDING" => Ok(Self::Pending),
            "PAID" => Ok(Self::Paid),
            "UNPAID" => Ok(Self::Unpaid),
            "ISSUED" => Ok(Self::Issued),
            _ => Err(Error::UnknownState),
        }
    }
}

/// Mint quote response [NUT-04]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
#[serde(bound = "Q: Serialize + DeserializeOwned")]
pub struct MintQuoteBolt11Response<Q> {
    /// Quote Id
    pub quote: Q,
    /// Payment request to fulfil
    pub request: String,
    /// Quote State
    pub state: MintQuoteState,
    /// Unix timestamp until the quote is valid
    pub expiry: Option<u64>,
    /// NUT-19 Pubkey
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pubkey: Option<PublicKey>,
}

impl<Q: ToString> MintQuoteBolt11Response<Q> {
    /// Convert the MintQuote with a quote type Q to a String
    pub fn to_string_id(&self) -> MintQuoteBolt11Response<String> {
        MintQuoteBolt11Response {
            quote: self.quote.to_string(),
            request: self.request.clone(),
            state: self.state,
            expiry: self.expiry,
            pubkey: self.pubkey,
        }
    }
}

#[cfg(feature = "mint")]
impl From<MintQuoteBolt11Response<Uuid>> for MintQuoteBolt11Response<String> {
    fn from(value: MintQuoteBolt11Response<Uuid>) -> Self {
        Self {
            quote: value.quote.to_string(),
            request: value.request,
            state: value.state,
            expiry: value.expiry,
            pubkey: value.pubkey,
        }
    }
}

#[cfg(feature = "mint")]
impl From<crate::mint::MintQuote> for MintQuoteBolt11Response<Uuid> {
    fn from(mint_quote: crate::mint::MintQuote) -> MintQuoteBolt11Response<Uuid> {
        MintQuoteBolt11Response {
            quote: mint_quote.id,
            request: mint_quote.request,
            state: mint_quote.state,
            expiry: Some(mint_quote.expiry),
            pubkey: mint_quote.pubkey,
        }
    }
}

/// Mint request [NUT-04]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
#[serde(bound = "Q: Serialize + DeserializeOwned")]
pub struct MintBolt11Request<Q> {
    /// Quote id
    #[cfg_attr(feature = "swagger", schema(max_length = 1_000))]
    pub quote: Q,
    /// Outputs
    #[cfg_attr(feature = "swagger", schema(max_items = 1_000))]
    pub outputs: Vec<BlindedMessage>,
    /// Signature
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

#[cfg(feature = "mint")]
impl TryFrom<MintBolt11Request<String>> for MintBolt11Request<Uuid> {
    type Error = uuid::Error;

    fn try_from(value: MintBolt11Request<String>) -> Result<Self, Self::Error> {
        Ok(Self {
            quote: Uuid::from_str(&value.quote)?,
            outputs: value.outputs,
            signature: value.signature,
        })
    }
}

impl<Q> MintBolt11Request<Q> {
    /// Total [`Amount`] of outputs
    pub fn total_amount(&self) -> Result<Amount, Error> {
        Amount::try_sum(
            self.outputs
                .iter()
                .map(|BlindedMessage { amount, .. }| *amount),
        )
        .map_err(|_| Error::AmountOverflow)
    }
}

/// Mint response [NUT-04]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct MintBolt11Response {
    /// Blinded Signatures
    pub signatures: Vec<BlindSignature>,
}

/// Mint Method Settings
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct MintMethodSettings {
    /// Payment Method e.g. bolt11
    pub method: PaymentMethod,
    /// Currency Unit e.g. sat
    pub unit: CurrencyUnit,
    /// Min Amount
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_amount: Option<Amount>,
    /// Max Amount
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_amount: Option<Amount>,
    /// Quote Description
    #[serde(default)]
    pub description: bool,
}

/// Mint Settings
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema), schema(as = nut04::Settings))]
pub struct Settings {
    /// Methods to mint
    pub methods: Vec<MintMethodSettings>,
    /// Minting disabled
    pub disabled: bool,
}

impl Settings {
    /// Create new [`Settings`]
    pub fn new(methods: Vec<MintMethodSettings>, disabled: bool) -> Self {
        Self { methods, disabled }
    }

    /// Get [`MintMethodSettings`] for unit method pair
    pub fn get_settings(
        &self,
        unit: &CurrencyUnit,
        method: &PaymentMethod,
    ) -> Option<MintMethodSettings> {
        for method_settings in self.methods.iter() {
            if method_settings.method.eq(method) && method_settings.unit.eq(unit) {
                return Some(method_settings.clone());
            }
        }

        None
    }
}

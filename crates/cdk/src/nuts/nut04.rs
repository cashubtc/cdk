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
use super::MintQuoteState;
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
    pub description: Option<String>,
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
}

#[cfg(feature = "mint")]
impl From<crate::mint::MintQuote> for MintQuoteBolt11Response<Uuid> {
    fn from(mint_quote: crate::mint::MintQuote) -> MintQuoteBolt11Response<Uuid> {
        MintQuoteBolt11Response {
            quote: mint_quote.id,
            request: mint_quote.request,
            state: mint_quote.state,
            expiry: Some(mint_quote.expiry),
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
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

impl Default for Settings {
    fn default() -> Self {
        let bolt11_mint = MintMethodSettings {
            method: PaymentMethod::Bolt11,
            unit: CurrencyUnit::Sat,
            min_amount: Some(Amount::from(1)),
            max_amount: Some(Amount::from(1000000)),
            description: true,
        };

        Settings {
            methods: vec![bolt11_mint],
            disabled: false,
        }
    }
}

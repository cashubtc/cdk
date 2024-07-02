//! NUT-04: Mint Tokens via Bolt11
//!
//! <https://github.com/cashubtc/nuts/blob/main/04.md>

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use thiserror::Error;

use super::nut00::{BlindSignature, BlindedMessage, CurrencyUnit, PaymentMethod};
use super::MintQuoteState;
use crate::types::MintQuote;
use crate::Amount;

/// NUT04 Error
#[derive(Debug, Error)]
pub enum Error {
    /// Unknown Quote State
    #[error("Unknown Quote State")]
    UnknownState,
}

/// Mint quote request [NUT-04]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintQuoteBolt11Request {
    /// Amount
    pub amount: Amount,
    /// Unit wallet would like to pay with
    pub unit: CurrencyUnit,
}

/// Possible states of a quote
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum QuoteState {
    /// Quote has not been paid
    #[default]
    Unpaid,
    /// Quote has been paid and wallet can mint
    Paid,
    /// Minting is in progress
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MintQuoteBolt11Response {
    /// Quote Id
    pub quote: String,
    /// Payment request to fulfil
    pub request: String,
    // TODO: To be deprecated
    /// Whether the the request haas be paid
    /// Deprecated
    pub paid: Option<bool>,
    /// Quote State
    pub state: MintQuoteState,
    /// Unix timestamp until the quote is valid
    pub expiry: Option<u64>,
}

// A custom deserializer is needed until all mints
// update some will return without the required state.
impl<'de> Deserialize<'de> for MintQuoteBolt11Response {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;

        let quote: String = serde_json::from_value(
            value
                .get("quote")
                .ok_or(serde::de::Error::missing_field("quote"))?
                .clone(),
        )
        .map_err(|_| serde::de::Error::custom("Invalid quote id string"))?;

        let request: String = serde_json::from_value(
            value
                .get("request")
                .ok_or(serde::de::Error::missing_field("request"))?
                .clone(),
        )
        .map_err(|_| serde::de::Error::custom("Invalid request string"))?;

        let paid: Option<bool> = value.get("paid").and_then(|p| p.as_bool());

        let state: Option<String> = value
            .get("state")
            .and_then(|s| serde_json::from_value(s.clone()).ok());

        let (state, paid) = match (state, paid) {
            (None, None) => return Err(serde::de::Error::custom("State or paid must be defined")),
            (Some(state), _) => {
                let state: QuoteState = QuoteState::from_str(&state)
                    .map_err(|_| serde::de::Error::custom("Unknown state"))?;
                let paid = state == QuoteState::Paid;

                (state, paid)
            }
            (None, Some(paid)) => {
                let state = if paid {
                    QuoteState::Paid
                } else {
                    QuoteState::Unpaid
                };
                (state, paid)
            }
        };

        let expiry = value
            .get("expiry")
            .ok_or(serde::de::Error::missing_field("expiry"))?
            .as_u64();

        Ok(Self {
            quote,
            request,
            paid: Some(paid),
            state,
            expiry,
        })
    }
}

impl From<MintQuote> for MintQuoteBolt11Response {
    fn from(mint_quote: MintQuote) -> MintQuoteBolt11Response {
        let paid = mint_quote.state == QuoteState::Paid;
        MintQuoteBolt11Response {
            quote: mint_quote.id,
            request: mint_quote.request,
            paid: Some(paid),
            state: mint_quote.state,
            expiry: Some(mint_quote.expiry),
        }
    }
}

/// Mint request [NUT-04]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintBolt11Request {
    /// Quote id
    pub quote: String,
    /// Outputs
    pub outputs: Vec<BlindedMessage>,
}

impl MintBolt11Request {
    /// Total [`Amount`] of outputs
    pub fn total_amount(&self) -> Amount {
        self.outputs
            .iter()
            .map(|BlindedMessage { amount, .. }| *amount)
            .sum()
    }
}

/// Mint response [NUT-04]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintBolt11Response {
    /// Blinded Signatures
    pub signatures: Vec<BlindSignature>,
}

/// Mint Method Settings
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MintMethodSettings {
    /// Payment Method e.g. bolt11
    method: PaymentMethod,
    /// Currency Unit e.g. sat
    unit: CurrencyUnit,
    /// Min Amount
    #[serde(skip_serializing_if = "Option::is_none")]
    min_amount: Option<Amount>,
    /// Max Amount
    #[serde(skip_serializing_if = "Option::is_none")]
    max_amount: Option<Amount>,
}

/// Mint Settings
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Settings {
    methods: Vec<MintMethodSettings>,
    disabled: bool,
}

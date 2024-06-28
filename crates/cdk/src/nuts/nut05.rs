//! NUT-05: Melting Tokens
//!
//! <https://github.com/cashubtc/nuts/blob/main/05.md>

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use thiserror::Error;

use super::nut00::{BlindSignature, BlindedMessage, CurrencyUnit, PaymentMethod, Proofs};
use super::nut15::Mpp;
use crate::types::MeltQuote;
use crate::{Amount, Bolt11Invoice};

/// NUT05 Error
#[derive(Debug, Error)]
pub enum Error {
    /// Unknown Quote State
    #[error("Unknown Quote State")]
    UnknownState,
}

/// Melt quote request [NUT-05]
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltQuoteBolt11Request {
    /// Bolt11 invoice to be paid
    pub request: Bolt11Invoice,
    /// Unit wallet would like to pay with
    pub unit: CurrencyUnit,
    /// Payment Options
    pub options: Option<Mpp>,
}

/// Possible states of a quote
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum QuoteState {
    /// Quote has not been paid
    #[default]
    Unpaid,
    /// Quote has been paid
    Paid,
    /// Paying quote is in progress
    Pending,
}

impl fmt::Display for QuoteState {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Unpaid => write!(f, "UNPAID"),
            Self::Paid => write!(f, "PAID"),
            Self::Pending => write!(f, "PENDING"),
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
            _ => Err(Error::UnknownState),
        }
    }
}

/// Melt quote response [NUT-05]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MeltQuoteBolt11Response {
    /// Quote Id
    pub quote: String,
    /// The amount that needs to be provided
    pub amount: Amount,
    /// The fee reserve that is required
    pub fee_reserve: Amount,
    /// Whether the the request haas be paid
    // TODO: To be deprecated
    /// Deprecated
    pub paid: Option<bool>,
    /// Quote State
    pub state: QuoteState,
    /// Unix timestamp until the quote is valid
    pub expiry: u64,
    /// Payment preimage
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_preimage: Option<String>,
    /// Change
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<Vec<BlindSignature>>,
}

// A custom deserializer is needed until all mints
// update some will return without the required state.
impl<'de> Deserialize<'de> for MeltQuoteBolt11Response {
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
        .map_err(|_| serde::de::Error::custom("Invalid quote if string"))?;

        let amount = value
            .get("amount")
            .ok_or(serde::de::Error::missing_field("amount"))?
            .as_u64()
            .ok_or(serde::de::Error::missing_field("amount"))?;
        let amount = Amount::from(amount);

        let fee_reserve = value
            .get("fee_reserve")
            .ok_or(serde::de::Error::missing_field("fee_reserve"))?
            .as_u64()
            .ok_or(serde::de::Error::missing_field("fee_reserve"))?;

        let fee_reserve = Amount::from(fee_reserve);

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
            .as_u64()
            .ok_or(serde::de::Error::missing_field("expiry"))?;

        let payment_preimage: Option<String> = value
            .get("payment_preimage")
            .and_then(|p| serde_json::from_value(p.clone()).ok());

        let change: Option<Vec<BlindSignature>> = value
            .get("change")
            .and_then(|b| serde_json::from_value(b.clone()).ok());

        Ok(Self {
            quote,
            amount,
            fee_reserve,
            paid: Some(paid),
            state,
            expiry,
            payment_preimage,
            change,
        })
    }
}

impl From<MeltQuote> for MeltQuoteBolt11Response {
    fn from(melt_quote: MeltQuote) -> MeltQuoteBolt11Response {
        let paid = melt_quote.state == QuoteState::Paid;
        MeltQuoteBolt11Response {
            quote: melt_quote.id,
            amount: melt_quote.amount,
            fee_reserve: melt_quote.fee_reserve,
            paid: Some(paid),
            state: melt_quote.state,
            expiry: melt_quote.expiry,
            payment_preimage: melt_quote.payment_preimage,
            change: None,
        }
    }
}

/// Melt Bolt11 Request [NUT-05]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltBolt11Request {
    /// Quote ID
    pub quote: String,
    /// Proofs
    pub inputs: Proofs,
    /// Blinded Message that can be used to return change [NUT-08]
    /// Amount field of BlindedMessages `SHOULD` be set to zero
    pub outputs: Option<Vec<BlindedMessage>>,
}

impl MeltBolt11Request {
    /// Total [`Amount`] of [`Proofs`]
    pub fn proofs_amount(&self) -> Amount {
        self.inputs.iter().map(|proof| proof.amount).sum()
    }
}

// TODO: to be deprecated
/// Melt Response [NUT-05]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltBolt11Response {
    /// Indicate if payment was successful
    pub paid: bool,
    /// Bolt11 preimage
    pub payment_preimage: Option<String>,
    /// Change
    pub change: Option<Vec<BlindSignature>>,
}

impl From<MeltQuoteBolt11Response> for MeltBolt11Response {
    fn from(quote_response: MeltQuoteBolt11Response) -> MeltBolt11Response {
        MeltBolt11Response {
            paid: quote_response.paid.unwrap(),
            payment_preimage: quote_response.payment_preimage,
            change: quote_response.change,
        }
    }
}

/// Melt Method Settings
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MeltMethodSettings {
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

/// Melt Settings
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Settings {
    methods: Vec<MeltMethodSettings>,
    disabled: bool,
}

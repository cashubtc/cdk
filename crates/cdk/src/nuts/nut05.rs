//! NUT-05: Melting Tokens
//!
//! <https://github.com/cashubtc/nuts/blob/main/05.md>

use std::fmt;
use std::str::FromStr;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use thiserror::Error;
#[cfg(feature = "mint")]
use uuid::Uuid;

use super::nut00::{BlindSignature, BlindedMessage, CurrencyUnit, PaymentMethod, Proofs};
use super::nut15::Mpp;
#[cfg(feature = "mint")]
use crate::mint::{self, MeltQuote};
use crate::nuts::MeltQuoteState;
use crate::{Amount, Bolt11Invoice};

/// NUT05 Error
#[derive(Debug, Error)]
pub enum Error {
    /// Unknown Quote State
    #[error("Unknown quote state")]
    UnknownState,
    /// Amount overflow
    #[error("Amount Overflow")]
    AmountOverflow,
}

/// Melt quote request [NUT-05]
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct MeltQuoteBolt11Request {
    /// Bolt11 invoice to be paid
    #[cfg_attr(feature = "swagger", schema(value_type = String))]
    pub request: Bolt11Invoice,
    /// Unit wallet would like to pay with
    pub unit: CurrencyUnit,
    /// Payment Options
    pub options: Option<Mpp>,
}

/// Possible states of a quote
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema), schema(as = MeltQuoteState))]
pub enum QuoteState {
    /// Quote has not been paid
    #[default]
    Unpaid,
    /// Quote has been paid
    Paid,
    /// Paying quote is in progress
    Pending,
    /// Unknown state
    Unknown,
    /// Failed
    Failed,
}

impl fmt::Display for QuoteState {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Unpaid => write!(f, "UNPAID"),
            Self::Paid => write!(f, "PAID"),
            Self::Pending => write!(f, "PENDING"),
            Self::Unknown => write!(f, "UNKNOWN"),
            Self::Failed => write!(f, "FAILED"),
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
            "UNKNOWN" => Ok(Self::Unknown),
            "FAILED" => Ok(Self::Failed),
            _ => Err(Error::UnknownState),
        }
    }
}

/// Melt quote response [NUT-05]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
#[serde(bound = "Q: Serialize")]
pub struct MeltQuoteBolt11Response<Q> {
    /// Quote Id
    pub quote: Q,
    /// The amount that needs to be provided
    pub amount: Amount,
    /// The fee reserve that is required
    pub fee_reserve: Amount,
    /// Whether the the request haas be paid
    // TODO: To be deprecated
    /// Deprecated
    pub paid: Option<bool>,
    /// Quote State
    pub state: MeltQuoteState,
    /// Unix timestamp until the quote is valid
    pub expiry: u64,
    /// Payment preimage
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_preimage: Option<String>,
    /// Change
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<Vec<BlindSignature>>,
}

#[cfg(feature = "mint")]
impl From<MeltQuoteBolt11Response<Uuid>> for MeltQuoteBolt11Response<String> {
    fn from(value: MeltQuoteBolt11Response<Uuid>) -> Self {
        Self {
            quote: value.quote.to_string(),
            amount: value.amount,
            fee_reserve: value.fee_reserve,
            paid: value.paid,
            state: value.state,
            expiry: value.expiry,
            payment_preimage: value.payment_preimage,
            change: value.change,
        }
    }
}

#[cfg(feature = "mint")]
impl From<&MeltQuote> for MeltQuoteBolt11Response<Uuid> {
    fn from(melt_quote: &MeltQuote) -> MeltQuoteBolt11Response<Uuid> {
        MeltQuoteBolt11Response {
            quote: melt_quote.id,
            payment_preimage: None,
            change: None,
            state: melt_quote.state,
            paid: Some(melt_quote.state == MeltQuoteState::Paid),
            expiry: melt_quote.expiry,
            amount: melt_quote.amount,
            fee_reserve: melt_quote.fee_reserve,
        }
    }
}

// A custom deserializer is needed until all mints
// update some will return without the required state.
impl<'de, Q: DeserializeOwned> Deserialize<'de> for MeltQuoteBolt11Response<Q> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;

        let quote: Q = serde_json::from_value(
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

#[cfg(feature = "mint")]
impl From<mint::MeltQuote> for MeltQuoteBolt11Response<Uuid> {
    fn from(melt_quote: mint::MeltQuote) -> MeltQuoteBolt11Response<Uuid> {
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
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
#[serde(bound = "Q: Serialize + DeserializeOwned")]
pub struct MeltBolt11Request<Q> {
    /// Quote ID
    pub quote: Q,
    /// Proofs
    #[cfg_attr(feature = "swagger", schema(value_type = Vec<Proof>))]
    pub inputs: Proofs,
    /// Blinded Message that can be used to return change [NUT-08]
    /// Amount field of BlindedMessages `SHOULD` be set to zero
    pub outputs: Option<Vec<BlindedMessage>>,
}

#[cfg(feature = "mint")]
impl TryFrom<MeltBolt11Request<String>> for MeltBolt11Request<Uuid> {
    type Error = uuid::Error;

    fn try_from(value: MeltBolt11Request<String>) -> Result<Self, Self::Error> {
        Ok(Self {
            quote: Uuid::from_str(&value.quote)?,
            inputs: value.inputs,
            outputs: value.outputs,
        })
    }
}

impl<Q: Serialize + DeserializeOwned> MeltBolt11Request<Q> {
    /// Total [`Amount`] of [`Proofs`]
    pub fn proofs_amount(&self) -> Result<Amount, Error> {
        Amount::try_sum(self.inputs.iter().map(|proof| proof.amount))
            .map_err(|_| Error::AmountOverflow)
    }
}

/// Melt Method Settings
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct MeltMethodSettings {
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
}

impl Settings {
    /// Create new [`Settings`]
    pub fn new(methods: Vec<MeltMethodSettings>, disabled: bool) -> Self {
        Self { methods, disabled }
    }

    /// Get [`MeltMethodSettings`] for unit method pair
    pub fn get_settings(
        &self,
        unit: &CurrencyUnit,
        method: &PaymentMethod,
    ) -> Option<MeltMethodSettings> {
        for method_settings in self.methods.iter() {
            if method_settings.method.eq(method) && method_settings.unit.eq(unit) {
                return Some(method_settings.clone());
            }
        }

        None
    }
}

/// Melt Settings
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema), schema(as = nut05::Settings))]
pub struct Settings {
    /// Methods to melt
    pub methods: Vec<MeltMethodSettings>,
    /// Minting disabled
    pub disabled: bool,
}

impl Default for Settings {
    fn default() -> Self {
        let bolt11_mint = MeltMethodSettings {
            method: PaymentMethod::Bolt11,
            unit: CurrencyUnit::Sat,
            min_amount: Some(Amount::from(1)),
            max_amount: Some(Amount::from(1000000)),
        };

        Settings {
            methods: vec![bolt11_mint],
            disabled: false,
        }
    }
}

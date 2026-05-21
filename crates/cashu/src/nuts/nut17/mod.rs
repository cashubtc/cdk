//! Specific Subscription for the cdk crate
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::PublicKey;
use crate::nut00::KnownMethod;
use crate::nut25::MeltQuoteBolt12Response;
use crate::nuts::{
    CurrencyUnit, MeltQuoteBolt11Response, MeltQuoteCustomResponse, MintQuoteBolt11Response,
    MintQuoteCustomResponse, PaymentMethod, ProofState,
};
use crate::quote_id::QuoteIdError;
use crate::MintQuoteBolt12Response;

pub mod ws;

/// Subscription Parameter according to the standard
#[derive(Debug, Clone, Serialize, Eq, PartialEq, Hash, Deserialize)]
#[serde(bound = "I: DeserializeOwned + Serialize")]
pub struct Params<I> {
    /// Kind
    pub kind: Kind,
    /// Filters
    pub filters: Vec<String>,
    /// Subscription Id
    #[serde(rename = "subId")]
    pub id: I,
}

/// Check state Settings
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct SupportedSettings {
    /// Supported methods
    pub supported: Vec<SupportedMethods>,
}

/// Supported WS Methods
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct SupportedMethods {
    /// Payment Method
    pub method: PaymentMethod,
    /// Unit
    pub unit: CurrencyUnit,
    /// Command
    pub commands: Vec<WsCommand>,
}

impl SupportedMethods {
    /// Create [`SupportedMethods`]
    pub fn new(method: PaymentMethod, unit: CurrencyUnit, commands: Vec<WsCommand>) -> Self {
        Self {
            method,
            unit,
            commands,
        }
    }

    /// Create [`SupportedMethods`] for Bolt11 with all supported commands
    pub fn default_bolt11(unit: CurrencyUnit) -> Self {
        let commands = vec![
            WsCommand::MintQuote,
            WsCommand::MeltQuote,
            WsCommand::ProofState,
        ];

        Self {
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            unit,
            commands,
        }
    }

    /// Create [`SupportedMethods`] for Bolt12 with all supported commands
    pub fn default_bolt12(unit: CurrencyUnit) -> Self {
        let commands = vec![
            WsCommand::MintQuote,
            WsCommand::MeltQuote,
            WsCommand::ProofState,
        ];

        Self {
            method: PaymentMethod::Known(KnownMethod::Bolt12),
            unit,
            commands,
        }
    }

    /// Create [`SupportedMethods`] for custom payment method with all supported commands
    pub fn default_custom(method: PaymentMethod, unit: CurrencyUnit) -> Self {
        let commands = vec![
            WsCommand::MintQuote,
            WsCommand::MeltQuote,
            WsCommand::ProofState,
        ];

        Self {
            method,
            unit,
            commands,
        }
    }
}

impl WsCommand {
    /// Create the generic mint quote command for any payment method
    pub fn custom_mint_quote(_method: &str) -> Self {
        Self::MintQuote
    }

    /// Create the generic melt quote command for any payment method
    pub fn custom_melt_quote(_method: &str) -> Self {
        Self::MeltQuote
    }
}

/// WebSocket commands supported by the Cashu mint
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub enum WsCommand {
    /// Command to subscribe to mint quote updates
    MintQuote,
    /// Command to subscribe to melt quote updates
    MeltQuote,
    #[deprecated(
        note = "Temporary backwards compatibility for legacy NUT-17 websocket commands. TODO: remove once old quote command strings are dropped."
    )]
    /// Command to request a Lightning invoice for minting tokens
    Bolt11MintQuote,
    #[deprecated(
        note = "Temporary backwards compatibility for legacy NUT-17 websocket commands. TODO: remove once old quote command strings are dropped."
    )]
    /// Command to request a Lightning payment for melting tokens
    Bolt11MeltQuote,
    #[deprecated(
        note = "Temporary backwards compatibility for legacy NUT-17 websocket commands. TODO: remove once old quote command strings are dropped."
    )]
    /// Websocket support for Bolt12 Mint Quote
    Bolt12MintQuote,
    #[deprecated(
        note = "Temporary backwards compatibility for legacy NUT-17 websocket commands. TODO: remove once old quote command strings are dropped."
    )]
    /// Websocket support for Bolt12 Melt Quote
    Bolt12MeltQuote,
    /// Command to check the state of a proof
    ProofState,
    /// Custom payment method command
    Custom(String),
}

impl Serialize for WsCommand {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let s = match self {
            WsCommand::MintQuote => "mint_quote",
            WsCommand::MeltQuote => "melt_quote",
            WsCommand::Bolt11MintQuote => "bolt11_mint_quote",
            WsCommand::Bolt11MeltQuote => "bolt11_melt_quote",
            WsCommand::Bolt12MintQuote => "bolt12_mint_quote",
            WsCommand::Bolt12MeltQuote => "bolt12_melt_quote",
            WsCommand::ProofState => "proof_state",
            WsCommand::Custom(custom) => custom.as_str(),
        };
        serializer.serialize_str(s)
    }
}

impl<'de> Deserialize<'de> for WsCommand {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(match s.as_str() {
            "mint_quote" => WsCommand::MintQuote,
            "melt_quote" => WsCommand::MeltQuote,
            "bolt11_mint_quote" => WsCommand::Bolt11MintQuote,
            "bolt11_melt_quote" => WsCommand::Bolt11MeltQuote,
            "bolt12_mint_quote" => WsCommand::Bolt12MintQuote,
            "bolt12_melt_quote" => WsCommand::Bolt12MeltQuote,
            "proof_state" => WsCommand::ProofState,
            custom => WsCommand::Custom(custom.to_string()),
        })
    }
}

impl<T> From<MintQuoteBolt12Response<T>> for NotificationPayload<T>
where
    T: Clone,
{
    fn from(mint_quote: MintQuoteBolt12Response<T>) -> NotificationPayload<T> {
        NotificationPayload::MintQuoteBolt12Response(mint_quote)
    }
}

impl<T> From<MeltQuoteBolt12Response<T>> for NotificationPayload<T>
where
    T: Clone,
{
    fn from(melt_quote: MeltQuoteBolt12Response<T>) -> NotificationPayload<T> {
        NotificationPayload::MeltQuoteBolt12Response(melt_quote)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound = "T: Serialize + DeserializeOwned")]
#[serde(untagged)]
/// Subscription response
pub enum NotificationPayload<T>
where
    T: Clone,
{
    /// Proof State
    ProofState(ProofState),
    /// Melt Quote Bolt11 Response
    MeltQuoteBolt11Response(MeltQuoteBolt11Response<T>),
    /// Mint Quote Bolt11 Response
    MintQuoteBolt11Response(MintQuoteBolt11Response<T>),
    /// Mint Quote Bolt12 Response
    MintQuoteBolt12Response(MintQuoteBolt12Response<T>),
    /// Melt Quote Bolt12 Response
    MeltQuoteBolt12Response(MeltQuoteBolt12Response<T>),
    /// Custom Mint Quote Response (method, response)
    CustomMintQuoteResponse(String, MintQuoteCustomResponse<T>),
    /// Custom Melt Quote Response (method, response)
    CustomMeltQuoteResponse(String, MeltQuoteCustomResponse<T>),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize, Hash, Serialize)]
#[serde(bound = "T: Serialize + DeserializeOwned")]
/// A parsed notification
pub enum NotificationId<T>
where
    T: Clone,
{
    /// ProofState id is a Pubkey
    ProofState(PublicKey),
    /// MintQuote id is a QuoteId, regardless of payment method
    MintQuote(T),
    /// MeltQuote id is a QuoteId, regardless of payment method
    MeltQuote(T),
    /// MeltQuote id is an QuoteId
    MeltQuoteBolt11(T),
    /// MintQuote id is an QuoteId
    MintQuoteBolt11(T),
    /// MintQuote id is an QuoteId
    MintQuoteBolt12(T),
    /// MintQuote id is an QuoteId
    MeltQuoteBolt12(T),
    /// MintQuote id is an QuoteId
    MintQuoteCustom(String, T),
    /// MintQuote id is an QuoteId
    MeltQuoteCustom(String, T),
}

/// Kind
#[derive(Debug, Clone, Eq, Ord, PartialOrd, PartialEq, Hash)]
pub enum Kind {
    /// Generic mint quote subscription kind
    MintQuote,
    /// Generic melt quote subscription kind
    MeltQuote,
    #[deprecated(
        note = "Temporary backwards compatibility for legacy NUT-17 websocket kinds. TODO: remove once old quote kind strings are dropped."
    )]
    /// Bolt 11 Melt Quote
    Bolt11MeltQuote,
    #[deprecated(
        note = "Temporary backwards compatibility for legacy NUT-17 websocket kinds. TODO: remove once old quote kind strings are dropped."
    )]
    /// Bolt 11 Mint Quote
    Bolt11MintQuote,
    /// Proof State
    ProofState,
    #[deprecated(
        note = "Temporary backwards compatibility for legacy NUT-17 websocket kinds. TODO: remove once old quote kind strings are dropped."
    )]
    /// Bolt 12 Mint Quote
    Bolt12MintQuote,
    #[deprecated(
        note = "Temporary backwards compatibility for legacy NUT-17 websocket kinds. TODO: remove once old quote kind strings are dropped."
    )]
    /// Bolt 12 Melt Quote
    Bolt12MeltQuote,
    /// Custom
    Custom(String),
}

impl Serialize for Kind {
    #[allow(deprecated)]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let s = match self {
            Kind::MintQuote => "mint_quote",
            Kind::MeltQuote => "melt_quote",
            Kind::Bolt11MintQuote => "bolt11_mint_quote",
            Kind::Bolt11MeltQuote => "bolt11_melt_quote",
            Kind::Bolt12MintQuote => "bolt12_mint_quote",
            Kind::Bolt12MeltQuote => "bolt12_melt_quote",
            Kind::ProofState => "proof_state",
            Kind::Custom(custom) => custom.as_str(),
        };
        serializer.serialize_str(s)
    }
}

impl<'de> Deserialize<'de> for Kind {
    #[allow(deprecated)]
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(match s.as_str() {
            "mint_quote" => Kind::MintQuote,
            "melt_quote" => Kind::MeltQuote,
            "bolt11_mint_quote" => Kind::Bolt11MintQuote,
            "bolt11_melt_quote" => Kind::Bolt11MeltQuote,
            "bolt12_mint_quote" => Kind::Bolt12MintQuote,
            "bolt12_melt_quote" => Kind::Bolt12MeltQuote,
            "proof_state" => Kind::ProofState,
            custom => Kind::Custom(custom.to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_generic_ws_commands() {
        assert_eq!(
            serde_json::to_string(&WsCommand::MintQuote).unwrap(),
            "\"mint_quote\""
        );
        assert_eq!(
            serde_json::to_string(&WsCommand::MeltQuote).unwrap(),
            "\"melt_quote\""
        );
        assert_eq!(
            serde_json::to_string(&Kind::MintQuote).unwrap(),
            "\"mint_quote\""
        );
        assert_eq!(
            serde_json::to_string(&Kind::MeltQuote).unwrap(),
            "\"melt_quote\""
        );
    }

    #[test]
    #[allow(deprecated)]
    fn deserialize_legacy_quote_kinds_for_compatibility() {
        assert_eq!(
            serde_json::from_str::<WsCommand>("\"bolt11_mint_quote\"").unwrap(),
            WsCommand::Bolt11MintQuote
        );
        assert_eq!(
            serde_json::from_str::<WsCommand>("\"bolt12_melt_quote\"").unwrap(),
            WsCommand::Bolt12MeltQuote
        );
        assert_eq!(
            serde_json::from_str::<Kind>("\"bolt11_mint_quote\"").unwrap(),
            Kind::Bolt11MintQuote
        );
        assert_eq!(
            serde_json::from_str::<Kind>("\"bolt12_melt_quote\"").unwrap(),
            Kind::Bolt12MeltQuote
        );
    }
}

impl<I> AsRef<I> for Params<I> {
    fn as_ref(&self) -> &I {
        &self.id
    }
}

/// Parsing error
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Uuid Error: {0}")]
    /// Uuid Error
    QuoteId(#[from] QuoteIdError),

    #[error("PublicKey Error: {0}")]
    /// PublicKey Error
    PublicKey(#[from] crate::nuts::nut01::Error),
}

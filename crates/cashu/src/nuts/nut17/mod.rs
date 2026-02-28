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
            WsCommand::Bolt11MintQuote,
            WsCommand::Bolt11MeltQuote,
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
            WsCommand::Bolt12MintQuote,
            WsCommand::Bolt12MeltQuote,
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
        let method_name = method.to_string();
        let commands = vec![
            WsCommand::Custom(format!("{}_mint_quote", method_name)),
            WsCommand::Custom(format!("{}_melt_quote", method_name)),
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
    /// Create a custom mint quote command for a payment method
    pub fn custom_mint_quote(method: &str) -> Self {
        WsCommand::Custom(format!("{}_mint_quote", method))
    }

    /// Create a custom melt quote command for a payment method
    pub fn custom_melt_quote(method: &str) -> Self {
        WsCommand::Custom(format!("{}_melt_quote", method))
    }
}

/// WebSocket commands supported by the Cashu mint
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub enum WsCommand {
    /// Command to request a Lightning invoice for minting tokens
    Bolt11MintQuote,
    /// Command to request a Lightning payment for melting tokens
    Bolt11MeltQuote,
    /// Websocket support for Bolt12 Mint Quote
    Bolt12MintQuote,
    /// Websocket support for Bolt12 Melt Quote
    Bolt12MeltQuote,
    /// Command to check the state of a proof
    ProofState,
    /// Custom payment method command
    Custom(String),
}

impl Serialize for WsCommand {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let s = match self {
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
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(match s.as_str() {
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
    /// Bolt 11 Melt Quote
    Bolt11MeltQuote,
    /// Bolt 11 Mint Quote
    Bolt11MintQuote,
    /// Proof State
    ProofState,
    /// Bolt 12 Mint Quote
    Bolt12MintQuote,
    /// Bolt 12 Melt Quote
    Bolt12MeltQuote,
    /// Custom
    Custom(String),
}

impl Serialize for Kind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let s = match self {
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
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(match s.as_str() {
            "bolt11_mint_quote" => Kind::Bolt11MintQuote,
            "bolt11_melt_quote" => Kind::Bolt11MeltQuote,
            "bolt12_mint_quote" => Kind::Bolt12MintQuote,
            "bolt12_melt_quote" => Kind::Bolt12MeltQuote,
            "proof_state" => Kind::ProofState,
            custom => Kind::Custom(custom.to_string()),
        })
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

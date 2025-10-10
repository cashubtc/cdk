//! Specific Subscription for the cdk crate
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::PublicKey;
use crate::nuts::{
    CurrencyUnit, MeltQuoteBolt11Response, MintQuoteBolt11Response, MintQuoteMiningShareResponse,
    PaymentMethod, ProofState,
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
            method: PaymentMethod::Bolt11,
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
            method: PaymentMethod::Bolt12,
            unit,
            commands,
        }
    }
}

/// WebSocket commands supported by the Cashu mint
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum WsCommand {
    /// Command to request a Lightning invoice for minting tokens
    #[serde(rename = "bolt11_mint_quote")]
    Bolt11MintQuote,
    /// Command to request a Lightning payment for melting tokens
    #[serde(rename = "bolt11_melt_quote")]
    Bolt11MeltQuote,
    /// Websocket support for Bolt12 Mint Quote
    #[serde(rename = "bolt12_mint_quote")]
    Bolt12MintQuote,
    /// Websocket support for Bolt12 Melt Quote
    #[serde(rename = "bolt12_melt_quote")]
    Bolt12MeltQuote,
    /// Command to check the state of a proof
    #[serde(rename = "proof_state")]
    ProofState,
}

impl<T> From<MintQuoteBolt12Response<T>> for NotificationPayload<T>
where
    T: Clone,
{
    fn from(mint_quote: MintQuoteBolt12Response<T>) -> NotificationPayload<T> {
        NotificationPayload::MintQuoteBolt12Response(mint_quote)
    }
}

impl<T> From<MintQuoteMiningShareResponse<T>> for NotificationPayload<T>
where
    T: Clone,
{
    fn from(mint_quote: MintQuoteMiningShareResponse<T>) -> NotificationPayload<T> {
        NotificationPayload::MintQuoteMiningShareResponse(mint_quote)
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
    /// Mint Quote Mining Share Response
    MintQuoteMiningShareResponse(MintQuoteMiningShareResponse<T>),
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
    /// MintQuote id is an QuoteId for mining share notifications
    MintQuoteMiningShare(T),
}

/// Kind
#[derive(Debug, Clone, Copy, Eq, Ord, PartialOrd, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Bolt 11 Melt Quote
    Bolt11MeltQuote,
    /// Bolt 11 Mint Quote
    Bolt11MintQuote,
    /// Proof State
    ProofState,
    /// Bolt 12 Mint Quote
    Bolt12MintQuote,
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

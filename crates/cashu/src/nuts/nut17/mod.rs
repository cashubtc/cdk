//! Specific Subscription for the cdk crate
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
#[cfg(feature = "mint")]
use uuid::Uuid;

#[cfg(feature = "mint")]
use super::PublicKey;
use crate::nuts::{
    CurrencyUnit, MeltQuoteBolt11Response, MintQuoteBolt11Response, PaymentMethod, ProofState,
};

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
pub struct SupportedSettings {
    /// Supported methods
    pub supported: Vec<SupportedMethods>,
}

/// Supported WS Methods
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SupportedMethods {
    /// Payment Method
    pub method: PaymentMethod,
    /// Unit
    pub unit: CurrencyUnit,
    /// Command
    pub commands: Vec<String>,
}

impl SupportedMethods {
    /// Create [`SupportedMethods`]
    pub fn new(method: PaymentMethod, unit: CurrencyUnit) -> Self {
        Self {
            method,
            unit,
            commands: Vec::new(),
        }
    }
}

impl Default for SupportedMethods {
    fn default() -> Self {
        SupportedMethods {
            method: PaymentMethod::Bolt11,
            unit: CurrencyUnit::Sat,
            commands: vec![
                "bolt11_mint_quote".to_owned(),
                "bolt11_melt_quote".to_owned(),
                "proof_state".to_owned(),
            ],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound = "T: Serialize + DeserializeOwned")]
#[serde(untagged)]
/// Subscription response
pub enum NotificationPayload<T> {
    /// Proof State
    ProofState(ProofState),
    /// Melt Quote Bolt11 Response
    MeltQuoteBolt11Response(MeltQuoteBolt11Response<T>),
    /// Mint Quote Bolt11 Response
    MintQuoteBolt11Response(MintQuoteBolt11Response<T>),
}

impl<T> From<ProofState> for NotificationPayload<T> {
    fn from(proof_state: ProofState) -> NotificationPayload<T> {
        NotificationPayload::ProofState(proof_state)
    }
}

impl<T> From<MeltQuoteBolt11Response<T>> for NotificationPayload<T> {
    fn from(melt_quote: MeltQuoteBolt11Response<T>) -> NotificationPayload<T> {
        NotificationPayload::MeltQuoteBolt11Response(melt_quote)
    }
}

impl<T> From<MintQuoteBolt11Response<T>> for NotificationPayload<T> {
    fn from(mint_quote: MintQuoteBolt11Response<T>) -> NotificationPayload<T> {
        NotificationPayload::MintQuoteBolt11Response(mint_quote)
    }
}

#[cfg(feature = "mint")]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
/// A parsed notification
pub enum Notification {
    /// ProofState id is a Pubkey
    ProofState(PublicKey),
    /// MeltQuote id is an Uuid
    MeltQuoteBolt11(Uuid),
    /// MintQuote id is an Uuid
    MintQuoteBolt11(Uuid),
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
}

impl<I> AsRef<I> for Params<I> {
    fn as_ref(&self) -> &I {
        &self.id
    }
}

/// Parsing error
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[cfg(feature = "mint")]
    #[error("Uuid Error: {0}")]
    /// Uuid Error
    Uuid(#[from] uuid::Error),

    #[error("PublicKey Error: {0}")]
    /// PublicKey Error
    PublicKey(#[from] crate::nuts::nut01::Error),
}

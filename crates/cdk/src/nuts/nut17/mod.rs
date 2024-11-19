//! Specific Subscription for the cdk crate
use std::str::FromStr;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::PublicKey;
use crate::nuts::{
    CurrencyUnit, MeltQuoteBolt11Response, MintQuoteBolt11Response, PaymentMethod, ProofState,
};
use crate::pub_sub::{Index, Indexable, SubscriptionGlobalId};

#[cfg(feature = "mint")]
mod manager;
#[cfg(feature = "mint")]
mod on_subscription;
#[cfg(feature = "mint")]
pub use manager::PubSubManager;
#[cfg(feature = "mint")]
pub use on_subscription::OnSubscription;

pub use crate::pub_sub::SubId;
pub mod ws;

/// Subscription Parameter according to the standard
#[derive(Debug, Clone, Serialize, Eq, PartialEq, Hash, Deserialize)]
pub struct Params {
    /// Kind
    pub kind: Kind,
    /// Filters
    pub filters: Vec<String>,
    /// Subscription Id
    #[serde(rename = "subId")]
    pub id: SubId,
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

impl Indexable for NotificationPayload<Uuid> {
    type Type = Notification;

    fn to_indexes(&self) -> Vec<Index<Self::Type>> {
        match self {
            NotificationPayload::ProofState(proof_state) => {
                vec![Index::from(Notification::ProofState(proof_state.y))]
            }
            NotificationPayload::MeltQuoteBolt11Response(melt_quote) => {
                vec![Index::from(Notification::MeltQuoteBolt11(melt_quote.quote))]
            }
            NotificationPayload::MintQuoteBolt11Response(mint_quote) => {
                vec![Index::from(Notification::MintQuoteBolt11(mint_quote.quote))]
            }
        }
    }
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

impl AsRef<SubId> for Params {
    fn as_ref(&self) -> &SubId {
        &self.id
    }
}

/// Parsing error
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Uuid Error: {0}")]
    /// Uuid Error
    Uuid(#[from] uuid::Error),

    #[error("PublicKey Error: {0}")]
    /// PublicKey Error
    PublicKey(#[from] crate::nuts::nut01::Error),
}

impl TryFrom<Params> for Vec<Index<Notification>> {
    type Error = Error;

    fn try_from(val: Params) -> Result<Self, Self::Error> {
        let sub_id: SubscriptionGlobalId = Default::default();
        val.filters
            .into_iter()
            .map(|filter| {
                let idx = match val.kind {
                    Kind::Bolt11MeltQuote => {
                        Notification::MeltQuoteBolt11(Uuid::from_str(&filter)?)
                    }
                    Kind::Bolt11MintQuote => {
                        Notification::MintQuoteBolt11(Uuid::from_str(&filter)?)
                    }
                    Kind::ProofState => Notification::ProofState(PublicKey::from_str(&filter)?),
                };

                Ok(Index::from((idx, val.id.clone(), sub_id)))
            })
            .collect::<Result<_, _>>()
    }
}

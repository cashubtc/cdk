//! Subscription-related FFI types
use std::sync::Arc;

use cdk::event::MintEvent;
use serde::{Deserialize, Serialize};

use super::proof::ProofStateUpdate;
use super::quote::{MeltQuoteBolt11Response, MintQuoteBolt11Response};
use crate::error::FfiError;

/// FFI-compatible SubscriptionKind
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum SubscriptionKind {
    /// Bolt 11 Melt Quote
    Bolt11MeltQuote,
    /// Bolt 11 Mint Quote
    Bolt11MintQuote,
    /// Bolt 12 Mint Quote
    Bolt12MintQuote,
    /// Proof State
    ProofState,
}

impl From<SubscriptionKind> for cdk::nuts::nut17::Kind {
    fn from(kind: SubscriptionKind) -> Self {
        match kind {
            SubscriptionKind::Bolt11MeltQuote => cdk::nuts::nut17::Kind::Bolt11MeltQuote,
            SubscriptionKind::Bolt11MintQuote => cdk::nuts::nut17::Kind::Bolt11MintQuote,
            SubscriptionKind::Bolt12MintQuote => cdk::nuts::nut17::Kind::Bolt12MintQuote,
            SubscriptionKind::ProofState => cdk::nuts::nut17::Kind::ProofState,
        }
    }
}

impl From<cdk::nuts::nut17::Kind> for SubscriptionKind {
    fn from(kind: cdk::nuts::nut17::Kind) -> Self {
        match kind {
            cdk::nuts::nut17::Kind::Bolt11MeltQuote => SubscriptionKind::Bolt11MeltQuote,
            cdk::nuts::nut17::Kind::Bolt11MintQuote => SubscriptionKind::Bolt11MintQuote,
            cdk::nuts::nut17::Kind::Bolt12MintQuote => SubscriptionKind::Bolt12MintQuote,
            cdk::nuts::nut17::Kind::ProofState => SubscriptionKind::ProofState,
        }
    }
}

/// FFI-compatible SubscribeParams
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct SubscribeParams {
    /// Subscription kind
    pub kind: SubscriptionKind,
    /// Filters
    pub filters: Vec<String>,
    /// Subscription ID (optional, will be generated if not provided)
    pub id: Option<String>,
}

impl From<SubscribeParams> for cdk::nuts::nut17::Params<Arc<String>> {
    fn from(params: SubscribeParams) -> Self {
        let sub_id = params.id.unwrap_or_else(|| {
            // Generate a random ID
            uuid::Uuid::new_v4().to_string()
        });

        cdk::nuts::nut17::Params {
            kind: params.kind.into(),
            filters: params.filters,
            id: Arc::new(sub_id),
        }
    }
}

impl SubscribeParams {
    /// Convert SubscribeParams to JSON string
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode SubscribeParams from JSON string
#[uniffi::export]
pub fn decode_subscribe_params(json: String) -> Result<SubscribeParams, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode SubscribeParams to JSON string
#[uniffi::export]
pub fn encode_subscribe_params(params: SubscribeParams) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&params)?)
}

/// FFI-compatible ActiveSubscription
#[derive(uniffi::Object)]
pub struct ActiveSubscription {
    inner: std::sync::Arc<tokio::sync::Mutex<cdk::wallet::subscription::ActiveSubscription>>,
    pub sub_id: String,
}

impl ActiveSubscription {
    pub(crate) fn new(
        inner: cdk::wallet::subscription::ActiveSubscription,
        sub_id: String,
    ) -> Self {
        Self {
            inner: std::sync::Arc::new(tokio::sync::Mutex::new(inner)),
            sub_id,
        }
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl ActiveSubscription {
    /// Get the subscription ID
    pub fn id(&self) -> String {
        self.sub_id.clone()
    }

    /// Receive the next notification
    pub async fn recv(&self) -> Result<NotificationPayload, FfiError> {
        let mut guard = self.inner.lock().await;
        guard
            .recv()
            .await
            .ok_or(FfiError::Generic {
                msg: "Subscription closed".to_string(),
            })
            .map(Into::into)
    }

    /// Try to receive a notification without blocking
    pub async fn try_recv(&self) -> Result<Option<NotificationPayload>, FfiError> {
        let mut guard = self.inner.lock().await;
        Ok(guard.try_recv().map(Into::into))
    }
}

/// FFI-compatible NotificationPayload
#[derive(Debug, Clone, uniffi::Enum)]
pub enum NotificationPayload {
    /// Proof state update
    ProofState { proof_states: Vec<ProofStateUpdate> },
    /// Mint quote update
    MintQuoteUpdate { quote: MintQuoteBolt11Response },
    /// Melt quote update
    MeltQuoteUpdate { quote: MeltQuoteBolt11Response },
}

impl From<MintEvent<String>> for NotificationPayload {
    fn from(payload: MintEvent<String>) -> Self {
        match payload.into() {
            cdk::nuts::NotificationPayload::ProofState(states) => NotificationPayload::ProofState {
                proof_states: vec![states.into()],
            },
            cdk::nuts::NotificationPayload::MintQuoteBolt11Response(quote_resp) => {
                NotificationPayload::MintQuoteUpdate {
                    quote: quote_resp.into(),
                }
            }
            cdk::nuts::NotificationPayload::MeltQuoteBolt11Response(quote_resp) => {
                NotificationPayload::MeltQuoteUpdate {
                    quote: quote_resp.into(),
                }
            }
            _ => {
                // For now, handle other notification types as empty ProofState
                NotificationPayload::ProofState {
                    proof_states: vec![],
                }
            }
        }
    }
}

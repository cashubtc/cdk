//! Subscription-related WASM types

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::proof::ProofStateUpdate;
use super::quote::{MeltQuoteBolt11Response, MintQuoteBolt11Response};
use crate::error::WasmError;

/// WASM-compatible SubscriptionKind
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubscriptionKind {
    Bolt11MeltQuote,
    Bolt11MintQuote,
    Bolt12MintQuote,
    Bolt12MeltQuote,
    ProofState,
}

impl From<SubscriptionKind> for cdk::nuts::nut17::Kind {
    fn from(kind: SubscriptionKind) -> Self {
        match kind {
            SubscriptionKind::Bolt11MeltQuote => cdk::nuts::nut17::Kind::Bolt11MeltQuote,
            SubscriptionKind::Bolt11MintQuote => cdk::nuts::nut17::Kind::Bolt11MintQuote,
            SubscriptionKind::Bolt12MintQuote => cdk::nuts::nut17::Kind::Bolt12MintQuote,
            SubscriptionKind::Bolt12MeltQuote => cdk::nuts::nut17::Kind::Bolt12MeltQuote,
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
            cdk::nuts::nut17::Kind::Bolt12MeltQuote => SubscriptionKind::Bolt12MeltQuote,
            cdk::nuts::nut17::Kind::ProofState => SubscriptionKind::ProofState,
            _ => SubscriptionKind::ProofState,
        }
    }
}

/// WASM-compatible SubscribeParams
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscribeParams {
    pub kind: SubscriptionKind,
    pub filters: Vec<String>,
    pub id: Option<String>,
}

impl From<SubscribeParams> for cdk::nuts::nut17::Params<Arc<String>> {
    fn from(params: SubscribeParams) -> Self {
        let sub_id = params
            .id
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        cdk::nuts::nut17::Params {
            kind: params.kind.into(),
            filters: params.filters,
            id: Arc::new(sub_id),
        }
    }
}

impl SubscribeParams {
    /// Convert SubscribeParams to JSON string
    pub fn to_json(&self) -> Result<String, WasmError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// WASM-compatible NotificationPayload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NotificationPayload {
    ProofState { proof_states: Vec<ProofStateUpdate> },
    MintQuoteUpdate { quote: MintQuoteBolt11Response },
    MeltQuoteUpdate { quote: MeltQuoteBolt11Response },
}

impl From<cdk::event::MintEvent<String>> for NotificationPayload {
    fn from(payload: cdk::event::MintEvent<String>) -> Self {
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
            _ => NotificationPayload::ProofState {
                proof_states: vec![],
            },
        }
    }
}

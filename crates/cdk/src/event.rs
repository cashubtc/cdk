//! Mint event types
use std::fmt::Debug;
use std::hash::Hash;
use std::ops::Deref;

use cdk_common::nut17::NotificationId;
use cdk_common::pub_sub::Event;
use cdk_common::{
    MeltQuoteBolt11Response, MintQuoteBolt11Response, MintQuoteBolt12Response, NotificationPayload,
    ProofState,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Simple wrapper over `NotificationPayload<QuoteId>` which is a foreign type
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(bound = "T: Serialize + DeserializeOwned")]
pub struct MintEvent<T>(NotificationPayload<T>)
where
    T: Clone + Eq + PartialEq;

impl<T> From<MintEvent<T>> for NotificationPayload<T>
where
    T: Clone + Eq + PartialEq,
{
    fn from(value: MintEvent<T>) -> Self {
        value.0
    }
}

impl<T> Deref for MintEvent<T>
where
    T: Clone + Eq + PartialEq,
{
    type Target = NotificationPayload<T>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> From<ProofState> for MintEvent<T>
where
    T: Clone + Eq + PartialEq,
{
    fn from(value: ProofState) -> Self {
        Self(NotificationPayload::ProofState(value))
    }
}

impl<T> MintEvent<T>
where
    T: Clone + Eq + PartialEq,
{
    /// New instance
    pub fn new(t: NotificationPayload<T>) -> Self {
        Self(t)
    }

    /// Get inner
    pub fn inner(&self) -> &NotificationPayload<T> {
        &self.0
    }

    /// Into inner
    pub fn into_inner(self) -> NotificationPayload<T> {
        self.0
    }
}

impl<T> From<NotificationPayload<T>> for MintEvent<T>
where
    T: Clone + Eq + PartialEq,
{
    fn from(value: NotificationPayload<T>) -> Self {
        Self(value)
    }
}

impl<T> From<MintQuoteBolt11Response<T>> for MintEvent<T>
where
    T: Clone + Eq + PartialEq,
{
    fn from(value: MintQuoteBolt11Response<T>) -> Self {
        Self(NotificationPayload::MintQuoteBolt11Response(value))
    }
}

impl<T> From<MeltQuoteBolt11Response<T>> for MintEvent<T>
where
    T: Clone + Eq + PartialEq,
{
    fn from(value: MeltQuoteBolt11Response<T>) -> Self {
        Self(NotificationPayload::MeltQuoteBolt11Response(value))
    }
}

impl<T> From<MintQuoteBolt12Response<T>> for MintEvent<T>
where
    T: Clone + Eq + PartialEq,
{
    fn from(value: MintQuoteBolt12Response<T>) -> Self {
        Self(NotificationPayload::MintQuoteBolt12Response(value))
    }
}

impl<T> Event for MintEvent<T>
where
    T: Clone + Serialize + DeserializeOwned + Debug + Ord + Hash + Send + Sync + Eq + PartialEq,
{
    type Topic = NotificationId<T>;

    fn get_topics(&self) -> Vec<Self::Topic> {
        match &self.0 {
            NotificationPayload::MeltQuoteBolt11Response(r) => {
                // HACK: MeltQuoteBolt12Response is a type alias for MeltQuoteBolt11Response.
                // Since NotificationPayload uses untagged serde, all melt responses are
                // deserialized as Bolt11. We broadcast to both topics to ensure Bolt12
                // subscribers receive the event.
                vec![
                    NotificationId::MeltQuoteBolt11(r.quote.to_owned()),
                    NotificationId::MeltQuoteBolt12(r.quote.to_owned()),
                ]
            }
            NotificationPayload::MintQuoteBolt11Response(r) => {
                vec![NotificationId::MintQuoteBolt11(r.quote.to_owned())]
            }
            NotificationPayload::MintQuoteBolt12Response(r) => {
                vec![NotificationId::MintQuoteBolt12(r.quote.to_owned())]
            }
            NotificationPayload::MeltQuoteBolt12Response(r) => {
                vec![NotificationId::MeltQuoteBolt12(r.quote.to_owned())]
            }
            NotificationPayload::CustomMintQuoteResponse(method, r) => {
                vec![NotificationId::MintQuoteCustom(
                    method.clone(),
                    r.quote.to_owned(),
                )]
            }
            NotificationPayload::CustomMeltQuoteResponse(method, r) => {
                vec![NotificationId::MeltQuoteCustom(
                    method.clone(),
                    r.quote.to_owned(),
                )]
            }
            NotificationPayload::ProofState(p) => vec![NotificationId::ProofState(p.y.to_owned())],
        }
    }
}

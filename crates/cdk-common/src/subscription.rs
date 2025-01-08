//! Subscription types and traits
use std::str::FromStr;

use cashu::nut17::{self, Error, Kind, Notification};
use cashu::{NotificationPayload, PublicKey};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::pub_sub::index::{Index, Indexable, SubscriptionGlobalId};
use crate::pub_sub::SubId;

/// Subscription parameters.
///
/// This is a concrete type alias for `nut17::Params<SubId>`.
pub type Params = nut17::Params<SubId>;

/// Wrapper around `nut17::Params` to implement `Indexable` for `Notification`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexableParams(Params);

impl From<Params> for IndexableParams {
    fn from(params: Params) -> Self {
        Self(params)
    }
}

impl TryFrom<IndexableParams> for Vec<Index<Notification>> {
    type Error = Error;
    fn try_from(params: IndexableParams) -> Result<Self, Self::Error> {
        let sub_id: SubscriptionGlobalId = Default::default();
        let params = params.0;
        params
            .filters
            .into_iter()
            .map(|filter| {
                let idx = match params.kind {
                    Kind::Bolt11MeltQuote => {
                        Notification::MeltQuoteBolt11(Uuid::from_str(&filter)?)
                    }
                    Kind::Bolt11MintQuote => {
                        Notification::MintQuoteBolt11(Uuid::from_str(&filter)?)
                    }
                    Kind::ProofState => Notification::ProofState(PublicKey::from_str(&filter)?),
                };

                Ok(Index::from((idx, params.id.clone(), sub_id)))
            })
            .collect::<Result<_, _>>()
    }
}

impl AsRef<SubId> for IndexableParams {
    fn as_ref(&self) -> &SubId {
        &self.0.id
    }
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

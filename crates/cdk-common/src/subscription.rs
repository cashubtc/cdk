//! Subscription types and traits
#[cfg(feature = "mint")]
use std::str::FromStr;

use cashu::nut17::{self};
#[cfg(feature = "mint")]
use cashu::nut17::{Error, Kind, Notification};
#[cfg(feature = "mint")]
use cashu::{NotificationPayload, PublicKey};
#[cfg(feature = "mint")]
use serde::{Deserialize, Serialize};
#[cfg(feature = "mint")]
use uuid::Uuid;

#[cfg(feature = "mint")]
use crate::pub_sub::index::{Index, Indexable, SubscriptionGlobalId};
use crate::pub_sub::SubId;

/// Subscription parameters.
///
/// This is a concrete type alias for `nut17::Params<SubId>`.
pub type Params = nut17::Params<SubId>;

/// Wrapper around `nut17::Params` to implement `Indexable` for `Notification`.
#[cfg(feature = "mint")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexableParams(Params);

#[cfg(feature = "mint")]
impl From<Params> for IndexableParams {
    fn from(params: Params) -> Self {
        Self(params)
    }
}

#[cfg(feature = "mint")]
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
                    Kind::Bolt12MintQuote => {
                        Notification::MintQuoteBolt12(Uuid::from_str(&filter)?)
                    }
                    Kind::OnchainMeltQuote => {
                        Notification::MeltQuoteOnchain(Uuid::from_str(&filter)?)
                    }
                    Kind::OnchainMintQuote => {
                        Notification::MintQuoteOnchain(Uuid::from_str(&filter)?)
                    }
                };

                Ok(Index::from((idx, params.id.clone(), sub_id)))
            })
            .collect::<Result<_, _>>()
    }
}

#[cfg(feature = "mint")]
impl AsRef<SubId> for IndexableParams {
    fn as_ref(&self) -> &SubId {
        &self.0.id
    }
}

#[cfg(feature = "mint")]
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
            NotificationPayload::MintQuoteBolt12Response(mint_quote) => {
                vec![Index::from(Notification::MintQuoteBolt12(mint_quote.quote))]
            }
            NotificationPayload::MintQuoteOnchainResponse(mint_quote) => {
                vec![Index::from(Notification::MintQuoteOnchain(
                    mint_quote.quote,
                ))]
            }
            NotificationPayload::MeltQuoteOnchainResponse(melt_quote) => {
                vec![Index::from(Notification::MeltQuoteOnchain(
                    melt_quote.quote,
                ))]
            }
        }
    }
}

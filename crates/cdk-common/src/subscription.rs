//! Subscription types and traits
use std::ops::Deref;
use std::str::FromStr;

use cashu::nut17::{self, Kind, NotificationId};
use cashu::quote_id::QuoteId;
use cashu::PublicKey;
use serde::{Deserialize, Serialize};

use crate::pub_sub::{Error, SubscriptionRequest};

/// Subscription parameters.
///
/// This is a concrete type alias for `nut17::Params<SubId>`.
pub type Params = nut17::Params<SubId>;

/// Wrapper around `nut17::Params` to implement `Indexable` for `Notification`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexableParams(Params);

#[cfg(feature = "mint")]
impl From<Params> for IndexableParams {
    fn from(params: Params) -> Self {
        Self(params)
    }
}

/// Subscription Id wrapper
///
/// This is the place to add some sane default (like a max length) to the
/// subscription ID
#[derive(Debug, Clone, Default, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct SubId(String);

impl From<&str> for SubId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for SubId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl FromStr for SubId {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.to_string()))
    }
}

impl Deref for SubId {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl SubscriptionRequest for IndexableParams {
    type Index = NotificationId;

    type SubscriptionName = SubId;

    fn subscription_name(&self) -> Self::SubscriptionName {
        self.0.id.clone()
    }

    fn try_get_indexes(&self) -> Result<Vec<Self::Index>, Error> {
        self.0
            .filters
            .iter()
            .map(|filter| match self.0.kind {
                Kind::Bolt11MeltQuote => QuoteId::from_str(filter)
                    .map(NotificationId::MeltQuoteBolt11)
                    .map_err(|_| Error::ParsingError(filter.to_owned())),
                Kind::Bolt11MintQuote => QuoteId::from_str(filter)
                    .map(NotificationId::MintQuoteBolt11)
                    .map_err(|_| Error::ParsingError(filter.to_owned())),
                Kind::ProofState => PublicKey::from_str(filter)
                    .map(NotificationId::ProofState)
                    .map_err(|_| Error::ParsingError(filter.to_owned())),

                Kind::Bolt12MintQuote => QuoteId::from_str(filter)
                    .map(NotificationId::MintQuoteBolt12)
                    .map_err(|_| Error::ParsingError(filter.to_owned())),
            })
            .collect::<Result<Vec<_>, _>>()
    }
}

/*
#[cfg(feature = "mint")]
impl TryFrom<IndexableParams> for Vec<Notification> {
    type Error = Error;
    fn try_from(params: IndexableParams) -> Result<Self, Self::Error> {
        let params = params.0;
        params
            .filters
            .into_iter()
            .map(|filter| {
                let idx = match params.kind {
                    Kind::Bolt11MeltQuote => {
                        Notification::MeltQuoteBolt11(QuoteId::from_str(&filter)?)
                    }
                    Kind::Bolt11MintQuote => {
                        Notification::MintQuoteBolt11(QuoteId::from_str(&filter)?)
                    }
                    Kind::ProofState => Notification::ProofState(PublicKey::from_str(&filter)?),
                    Kind::Bolt12MintQuote => {
                        Notification::MintQuoteBolt12(QuoteId::from_str(&filter)?)
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
impl Indexable for NotificationPayload<QuoteId> {
    type Index = Notification;

    fn to_indexes(&self) -> Vec<Index<Self::Index>> {
        match self {
            NotificationPayload::ProofState(proof_state) => {
                vec![Index::from(Notification::ProofState(proof_state.y))]
            }
            NotificationPayload::MeltQuoteBolt11Response(melt_quote) => {
                vec![Index::from(Notification::MeltQuoteBolt11(
                    melt_quote.quote.clone(),
                ))]
            }
            NotificationPayload::MintQuoteBolt11Response(mint_quote) => {
                vec![Index::from(Notification::MintQuoteBolt11(
                    mint_quote.quote.clone(),
                ))]
            }
            NotificationPayload::MintQuoteBolt12Response(mint_quote) => {
                vec![Index::from(Notification::MintQuoteBolt12(
                    mint_quote.quote.clone(),
                ))]
            }
        }
    }
}
*/

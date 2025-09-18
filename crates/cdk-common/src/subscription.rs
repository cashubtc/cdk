//! Subscription types and traits
use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;

use cashu::nut17::{self, Kind, NotificationId};
use cashu::quote_id::QuoteId;
use cashu::PublicKey;
use serde::{Deserialize, Serialize};

use crate::pub_sub::{Error, SubscriptionRequest};

/// CDK/Mint Subscription parameters.
///
/// This is a concrete type alias for `nut17::Params<SubId>`.
pub type Params = nut17::Params<Arc<SubId>>;

impl SubscriptionRequest for Params {
    type Topic = NotificationId<QuoteId>;

    type SubscriptionId = SubId;

    fn subscription_name(&self) -> Arc<Self::SubscriptionId> {
        self.id.clone()
    }

    fn try_get_topics(&self) -> Result<Vec<Self::Topic>, Error> {
        self.filters
            .iter()
            .map(|filter| match self.kind {
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

/// Subscriptions parameters for the wallet
///
/// This is because the Wallet can subscribe to non CDK quotes, where IDs are not constraint to
/// QuoteId
pub type WalletParams = nut17::Params<Arc<String>>;

impl SubscriptionRequest for WalletParams {
    type Topic = NotificationId<String>;

    type SubscriptionId = String;

    fn subscription_name(&self) -> Arc<Self::SubscriptionId> {
        self.id.clone()
    }

    fn try_get_topics(&self) -> Result<Vec<Self::Topic>, Error> {
        self.filters
            .iter()
            .map(|filter| {
                Ok(match self.kind {
                    Kind::Bolt11MeltQuote => NotificationId::MeltQuoteBolt11(filter.to_owned()),
                    Kind::Bolt11MintQuote => NotificationId::MintQuoteBolt11(filter.to_owned()),
                    Kind::ProofState => PublicKey::from_str(filter)
                        .map(NotificationId::ProofState)
                        .map_err(|_| Error::ParsingError(filter.to_owned()))?,

                    Kind::Bolt12MintQuote => NotificationId::MintQuoteBolt12(filter.to_owned()),
                })
            })
            .collect::<Result<Vec<_>, _>>()
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

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
        let mut topics = Vec::new();

        for filter in &self.filters {
            match self.kind {
                Kind::Bolt11MeltQuote => {
                    let id = QuoteId::from_str(filter)
                        .map_err(|_| Error::ParsingError(filter.to_owned()))?;
                    topics.push(NotificationId::MeltQuoteBolt11(id));
                }
                Kind::Bolt11MintQuote => {
                    let id = QuoteId::from_str(filter)
                        .map_err(|_| Error::ParsingError(filter.to_owned()))?;
                    topics.push(NotificationId::MintQuoteBolt11(id));
                }
                Kind::ProofState => {
                    let pk = PublicKey::from_str(filter)
                        .map_err(|_| Error::ParsingError(filter.to_owned()))?;
                    topics.push(NotificationId::ProofState(pk));
                }
                Kind::Bolt12MintQuote => {
                    let id = QuoteId::from_str(filter)
                        .map_err(|_| Error::ParsingError(filter.to_owned()))?;
                    topics.push(NotificationId::MintQuoteBolt12(id));
                }
                Kind::MiningShareMintQuote => {
                    let id = QuoteId::from_str(filter)
                        .map_err(|_| Error::ParsingError(filter.to_owned()))?;
                    topics.push(NotificationId::MintQuoteMiningShare(id));
                }
            }
        }

        Ok(topics)
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
        let mut topics = Vec::new();

        for filter in &self.filters {
            match self.kind {
                Kind::Bolt11MeltQuote => {
                    topics.push(NotificationId::MeltQuoteBolt11(filter.to_owned()));
                }
                Kind::Bolt11MintQuote => {
                    topics.push(NotificationId::MintQuoteBolt11(filter.to_owned()));
                }
                Kind::ProofState => {
                    let pk = PublicKey::from_str(filter)
                        .map_err(|_| Error::ParsingError(filter.to_owned()))?;
                    topics.push(NotificationId::ProofState(pk));
                }
                Kind::Bolt12MintQuote => {
                    topics.push(NotificationId::MintQuoteBolt12(filter.to_owned()));
                }
                Kind::MiningShareMintQuote => {
                    topics.push(NotificationId::MintQuoteMiningShare(filter.to_owned()));
                }
            }
        }

        Ok(topics)
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

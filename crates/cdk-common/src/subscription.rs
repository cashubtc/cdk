//! Subscription types and traits
use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;

use cashu::nut17::{
    self, Kind, NotificationId, MAX_CUSTOM_KIND_LEN, MAX_FILTER_LEN, MAX_SUBSCRIPTION_ID_LEN,
};
use cashu::quote_id::QuoteId;
use cashu::PublicKey;
use serde::de::Error as DeError;
use serde::{Deserialize, Deserializer, Serialize};

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
        validate_retained_strings(&self.kind, &self.filters, &self.id)?;

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
                Kind::Bolt12MeltQuote => QuoteId::from_str(filter)
                    .map(NotificationId::MeltQuoteBolt12)
                    .map_err(|_| Error::ParsingError(filter.to_owned())),
                Kind::OnchainMintQuote => QuoteId::from_str(filter)
                    .map(NotificationId::MintQuoteOnchain)
                    .map_err(|_| Error::ParsingError(filter.to_owned())),
                Kind::OnchainMeltQuote => QuoteId::from_str(filter)
                    .map(NotificationId::MeltQuoteOnchain)
                    .map_err(|_| Error::ParsingError(filter.to_owned())),
                Kind::Custom(ref s) => {
                    if let Some(method) = s.strip_suffix("_mint_quote") {
                        QuoteId::from_str(filter)
                            .map(|id| NotificationId::MintQuoteCustom(method.to_string(), id))
                            .map_err(|_| Error::ParsingError(filter.to_owned()))
                    } else if let Some(method) = s.strip_suffix("_melt_quote") {
                        QuoteId::from_str(filter)
                            .map(|id| NotificationId::MeltQuoteCustom(method.to_string(), id))
                            .map_err(|_| Error::ParsingError(filter.to_owned()))
                    } else {
                        Err(Error::ParsingError(filter.to_owned()))
                    }
                }
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
        validate_retained_strings(&self.kind, &self.filters, &self.id)?;

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
                    Kind::Bolt12MeltQuote => NotificationId::MeltQuoteBolt12(filter.to_owned()),
                    Kind::OnchainMintQuote => NotificationId::MintQuoteOnchain(filter.to_owned()),
                    Kind::OnchainMeltQuote => NotificationId::MeltQuoteOnchain(filter.to_owned()),
                    Kind::Custom(ref s) => {
                        if let Some(method) = s.strip_suffix("_mint_quote") {
                            NotificationId::MintQuoteCustom(method.to_string(), filter.to_owned())
                        } else if let Some(method) = s.strip_suffix("_melt_quote") {
                            NotificationId::MeltQuoteCustom(method.to_string(), filter.to_owned())
                        } else {
                            // If we can't parse the custom method, we can't create a NotificationId
                            // This might happen if the custom kind doesn't follow the convention
                            return Err(Error::ParsingError(format!("Invalid custom kind: {}", s)));
                        }
                    }
                })
            })
            .collect::<Result<Vec<_>, _>>()
    }
}

fn validate_retained_strings(
    kind: &Kind,
    filters: &[String],
    subscription_id: &str,
) -> Result<(), Error> {
    if subscription_id.len() > MAX_SUBSCRIPTION_ID_LEN {
        return Err(Error::ParsingError(format!(
            "subscription ID exceeds {MAX_SUBSCRIPTION_ID_LEN} bytes"
        )));
    }

    if matches!(kind, Kind::Custom(custom) if custom.len() > MAX_CUSTOM_KIND_LEN) {
        return Err(Error::ParsingError(format!(
            "custom subscription kind exceeds {MAX_CUSTOM_KIND_LEN} bytes"
        )));
    }

    if filters.iter().any(|filter| filter.len() > MAX_FILTER_LEN) {
        return Err(Error::ParsingError(format!(
            "subscription filter exceeds {MAX_FILTER_LEN} bytes"
        )));
    }

    Ok(())
}

/// Subscription Id wrapper
///
#[derive(Debug, Clone, Default, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize)]
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
        Ok(Self::from(s))
    }
}

impl<'de> Deserialize<'de> for SubId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let id = String::deserialize(deserializer)?;
        if id.len() > MAX_SUBSCRIPTION_ID_LEN {
            return Err(D::Error::custom(format!(
                "subscription ID exceeds {MAX_SUBSCRIPTION_ID_LEN} bytes"
            )));
        }

        Ok(Self(id))
    }
}

impl Deref for SubId {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscription_id_length_is_bounded() {
        let max_length_id = "a".repeat(MAX_SUBSCRIPTION_ID_LEN);
        let sub_id: SubId = serde_json::from_value(serde_json::json!(max_length_id.clone()))
            .expect("maximum-length subscription ID");
        assert_eq!(&*sub_id, &max_length_id);
        assert_eq!(
            serde_json::to_value(&sub_id).expect("serialize subscription ID"),
            serde_json::json!(max_length_id)
        );

        let oversized_id = "a".repeat(MAX_SUBSCRIPTION_ID_LEN + 1);
        assert!(serde_json::from_value::<SubId>(serde_json::json!(oversized_id)).is_err());
    }

    #[test]
    fn oversized_filter_is_rejected_before_parsing() {
        let params = Params {
            kind: Kind::Bolt11MintQuote,
            filters: vec!["A".repeat(MAX_FILTER_LEN + 4)],
            id: Arc::new(SubId::from("subscription")),
        };

        let err = params.try_get_topics().expect_err("oversized filter");
        assert_eq!(
            err.to_string(),
            format!("Parsing Error subscription filter exceeds {MAX_FILTER_LEN} bytes")
        );
    }

    #[test]
    fn maximum_length_wallet_filter_is_accepted() {
        let filter = "a".repeat(MAX_FILTER_LEN);
        let params = WalletParams {
            kind: Kind::Bolt11MintQuote,
            filters: vec![filter.clone()],
            id: Arc::new("subscription".to_string()),
        };

        assert_eq!(
            params.try_get_topics().expect("maximum-length filter"),
            vec![NotificationId::MintQuoteBolt11(filter)]
        );
    }

    #[test]
    fn programmatically_constructed_oversized_custom_kind_is_rejected() {
        let params = Params {
            kind: Kind::Custom("a".repeat(MAX_CUSTOM_KIND_LEN + 1)),
            filters: vec![QuoteId::new().to_string()],
            id: Arc::new(SubId::from("subscription")),
        };

        let err = params.try_get_topics().expect_err("oversized custom kind");
        assert_eq!(
            err.to_string(),
            format!("Parsing Error custom subscription kind exceeds {MAX_CUSTOM_KIND_LEN} bytes")
        );
    }

    #[test]
    fn programmatically_constructed_oversized_subscription_id_is_rejected() {
        let params = Params {
            kind: Kind::Bolt11MintQuote,
            filters: vec![QuoteId::new().to_string()],
            id: Arc::new(SubId::from("a".repeat(MAX_SUBSCRIPTION_ID_LEN + 1))),
        };

        let err = params
            .try_get_topics()
            .expect_err("oversized subscription ID");
        assert_eq!(
            err.to_string(),
            format!("Parsing Error subscription ID exceeds {MAX_SUBSCRIPTION_ID_LEN} bytes")
        );

        let wallet_params = WalletParams {
            kind: Kind::Bolt11MintQuote,
            filters: vec!["quote-id".to_string()],
            id: Arc::new("a".repeat(MAX_SUBSCRIPTION_ID_LEN + 1)),
        };
        assert!(wallet_params.try_get_topics().is_err());
    }
}

//! Specific Subscription for the cdk crate
use super::{MeltQuoteBolt11Response, MintQuoteBolt11Response};
use crate::{
    nuts::ProofState,
    subscription::{self, Index, Indexable},
};
use serde::{Deserialize, Serialize};

/// Subscription Parameter according to the standard
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Params {
    /// Kind
    pub kind: Kind,
    /// Filters
    pub filters: Vec<String>,
    /// Subscription Id
    #[serde(rename = "subId")]
    pub id: SubId,
}

pub use crate::subscription::SubId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
/// Subscription response
pub enum SubscriptionResponse {
    /// Proof State
    ProofState(ProofState),
    /// Melt Quote Bolt11 Response
    MeltQuoteBolt11Response(MeltQuoteBolt11Response),
    /// Mint Quote Bolt11 Response
    MintQuoteBolt11Response(MintQuoteBolt11Response),
}

impl From<ProofState> for SubscriptionResponse {
    fn from(proof_state: ProofState) -> SubscriptionResponse {
        SubscriptionResponse::ProofState(proof_state)
    }
}

impl From<MeltQuoteBolt11Response> for SubscriptionResponse {
    fn from(melt_quote: MeltQuoteBolt11Response) -> SubscriptionResponse {
        SubscriptionResponse::MeltQuoteBolt11Response(melt_quote)
    }
}

impl From<MintQuoteBolt11Response> for SubscriptionResponse {
    fn from(mint_quote: MintQuoteBolt11Response) -> SubscriptionResponse {
        SubscriptionResponse::MintQuoteBolt11Response(mint_quote)
    }
}

impl Indexable for SubscriptionResponse {
    type Type = (String, Kind);

    fn to_indexes(&self) -> Vec<Index<Self::Type>> {
        match self {
            SubscriptionResponse::ProofState(proof_state) => {
                vec![Index::from((proof_state.y.to_hex(), Kind::ProofState))]
            }
            SubscriptionResponse::MeltQuoteBolt11Response(melt_quote) => {
                vec![Index::from((
                    melt_quote.quote.clone(),
                    Kind::Bolt11MeltQuote,
                ))]
            }
            SubscriptionResponse::MintQuoteBolt11Response(mint_quote) => {
                vec![Index::from((
                    mint_quote.quote.clone(),
                    Kind::Bolt11MintQuote,
                ))]
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, Ord, PartialOrd, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]

/// Kind
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

impl From<Params> for Vec<Index<(String, Kind)>> {
    fn from(val: Params) -> Self {
        val.filters
            .iter()
            .map(|filter| Index::from(((filter.clone(), val.kind), val.id.clone())))
            .collect()
    }
}

/// Manager
pub type Manager = subscription::Manager<SubscriptionResponse, (String, Kind)>;

#[cfg(test)]
mod test {
    use crate::nuts::{PublicKey, State};

    use super::*;
    use std::time::Duration;
    use tokio::time::sleep;

    #[tokio::test]
    async fn active_and_drop() {
        let manager = Manager::default();
        let params = Params {
            kind: Kind::ProofState,
            filters: vec!["x".to_string()],
            id: "uno".into(),
        };

        // Although the same param is used, two subscriptions are created, that
        // is because each index is unique, thanks to `Unique`, it is the
        // responsability of the implementor to make sure that SubId are unique
        // either globally or per client
        let subscriptions = vec![
            manager.subscribe(params.clone()).await,
            manager.subscribe(params).await,
        ];
        assert_eq!(2, manager.active_subscriptions());
        drop(subscriptions);

        sleep(Duration::from_millis(10)).await;

        assert_eq!(0, manager.active_subscriptions());
    }

    #[tokio::test]
    async fn broadcast() {
        let manager = Manager::default();
        let mut subscriptions = [
            manager
                .subscribe(Params {
                    kind: Kind::ProofState,
                    filters: vec![
                        "02194603ffa36356f4a56b7df9371fc3192472351453ec7398b8da8117e7c3e104"
                            .to_string(),
                    ],
                    id: "uno".into(),
                })
                .await,
            manager
                .subscribe(Params {
                    kind: Kind::ProofState,
                    filters: vec![
                        "02194603ffa36356f4a56b7df9371fc3192472351453ec7398b8da8117e7c3e104"
                            .to_string(),
                    ],
                    id: "dos".into(),
                })
                .await,
        ];

        let event = ProofState {
            y: PublicKey::from_hex(
                "02194603ffa36356f4a56b7df9371fc3192472351453ec7398b8da8117e7c3e104",
            )
            .expect("valid pk"),
            state: State::Pending,
            witness: None,
        };

        manager.broadcast(event.into());

        sleep(Duration::from_millis(10)).await;

        let (sub1, _) = subscriptions[0].try_recv().expect("valid message");
        assert_eq!("uno", *sub1);

        let (sub1, _) = subscriptions[1].try_recv().expect("valid message");
        assert_eq!("dos", *sub1);

        assert!(subscriptions[0].try_recv().is_err());
        assert!(subscriptions[1].try_recv().is_err());
    }

    #[test]
    fn parsing_request() {
        let json = r#"{"kind":"proof_state","filters":["x"],"subId":"uno"}"#;
        let params: Params = serde_json::from_str(json).expect("valid json");
        assert_eq!(params.kind, Kind::ProofState);
        assert_eq!(params.filters, vec!["x"]);
        assert_eq!(*params.id, "uno");
    }

    #[tokio::test]
    async fn json_test() {
        let manager = Manager::default();
        let mut subscription = manager
            .subscribe::<Params>(
                serde_json::from_str(r#"{"kind":"proof_state","filters":["02194603ffa36356f4a56b7df9371fc3192472351453ec7398b8da8117e7c3e104"],"subId":"uno"}"#)
                    .expect("valid json"),
            )
            .await;

        manager.broadcast(
            ProofState {
                y: PublicKey::from_hex(
                    "02194603ffa36356f4a56b7df9371fc3192472351453ec7398b8da8117e7c3e104",
                )
                .expect("valid pk"),
                state: State::Pending,
                witness: None,
            }
            .into(),
        );

        // no one is listening for this event
        manager.broadcast(
            ProofState {
                y: PublicKey::from_hex(
                    "020000000000000000000000000000000000000000000000000000000000000001",
                )
                .expect("valid pk"),
                state: State::Pending,
                witness: None,
            }
            .into(),
        );

        sleep(Duration::from_millis(10)).await;
        let (sub1, msg) = subscription.try_recv().expect("valid message");
        assert_eq!("uno", *sub1);
        assert_eq!(
            r#"{"Y":"02194603ffa36356f4a56b7df9371fc3192472351453ec7398b8da8117e7c3e104","state":"PENDING","witness":null}"#,
            serde_json::to_string(&msg).expect("valid json")
        );
        assert!(subscription.try_recv().is_err());
    }
}

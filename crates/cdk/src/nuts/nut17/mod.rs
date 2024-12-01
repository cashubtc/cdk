//! Specific Subscription for the cdk crate

use std::ops::Deref;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

mod on_subscription;

pub use on_subscription::OnSubscription;

use crate::cdk_database::{self, MintDatabase};
use crate::nuts::{
    BlindSignature, CurrencyUnit, MeltQuoteBolt11Response, MeltQuoteState, MintQuoteBolt11Response,
    MintQuoteState, PaymentMethod, ProofState,
};
pub use crate::pub_sub::SubId;
use crate::pub_sub::{self, Index, Indexable, SubscriptionGlobalId};

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

/// Check state Settings
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SupportedSettings {
    /// Supported methods
    pub supported: Vec<SupportedMethods>,
}

impl Default for SupportedSettings {
    fn default() -> Self {
        SupportedSettings {
            supported: vec![SupportedMethods::default()],
        }
    }
}

/// Supported WS Methods
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SupportedMethods {
    /// Payment Method
    pub method: PaymentMethod,
    /// Unit
    pub unit: CurrencyUnit,
    /// Command
    pub commands: Vec<String>,
}

impl SupportedMethods {
    /// Create [`SupportedMethods`]
    pub fn new(method: PaymentMethod, unit: CurrencyUnit) -> Self {
        Self {
            method,
            unit,
            commands: vec![
                "bolt11_mint_quote".to_owned(),
                "bolt11_melt_quote".to_owned(),
                "proof_state".to_owned(),
            ],
        }
    }
}

impl Default for SupportedMethods {
    fn default() -> Self {
        SupportedMethods {
            method: PaymentMethod::Bolt11,
            unit: CurrencyUnit::Sat,
            commands: vec![
                "bolt11_mint_quote".to_owned(),
                "bolt11_melt_quote".to_owned(),
                "proof_state".to_owned(),
            ],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
/// Subscription response
pub enum NotificationPayload {
    /// Proof State
    ProofState(ProofState),
    /// Melt Quote Bolt11 Response
    MeltQuoteBolt11Response(MeltQuoteBolt11Response),
    /// Mint Quote Bolt11 Response
    MintQuoteBolt11Response(MintQuoteBolt11Response),
}

impl From<ProofState> for NotificationPayload {
    fn from(proof_state: ProofState) -> NotificationPayload {
        NotificationPayload::ProofState(proof_state)
    }
}

impl From<MeltQuoteBolt11Response> for NotificationPayload {
    fn from(melt_quote: MeltQuoteBolt11Response) -> NotificationPayload {
        NotificationPayload::MeltQuoteBolt11Response(melt_quote)
    }
}

impl From<MintQuoteBolt11Response> for NotificationPayload {
    fn from(mint_quote: MintQuoteBolt11Response) -> NotificationPayload {
        NotificationPayload::MintQuoteBolt11Response(mint_quote)
    }
}

impl Indexable for NotificationPayload {
    type Type = (String, Kind);

    fn to_indexes(&self) -> Vec<Index<Self::Type>> {
        match self {
            NotificationPayload::ProofState(proof_state) => {
                vec![Index::from((proof_state.y.to_hex(), Kind::ProofState))]
            }
            NotificationPayload::MeltQuoteBolt11Response(melt_quote) => {
                vec![Index::from((
                    melt_quote.quote.clone(),
                    Kind::Bolt11MeltQuote,
                ))]
            }
            NotificationPayload::MintQuoteBolt11Response(mint_quote) => {
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
        let sub_id: SubscriptionGlobalId = Default::default();
        val.filters
            .iter()
            .map(|filter| Index::from(((filter.clone(), val.kind), val.id.clone(), sub_id)))
            .collect()
    }
}

/// Manager
/// Publish–subscribe manager
///
/// Nut-17 implementation is system-wide and not only through the WebSocket, so
/// it is possible for another part of the system to subscribe to events.
pub struct PubSubManager(pub_sub::Manager<NotificationPayload, (String, Kind), OnSubscription>);

#[allow(clippy::default_constructed_unit_structs)]
impl Default for PubSubManager {
    fn default() -> Self {
        PubSubManager(OnSubscription::default().into())
    }
}

impl From<Arc<dyn MintDatabase<Err = cdk_database::Error> + Send + Sync>> for PubSubManager {
    fn from(val: Arc<dyn MintDatabase<Err = cdk_database::Error> + Send + Sync>) -> Self {
        PubSubManager(OnSubscription(Some(val)).into())
    }
}

impl Deref for PubSubManager {
    type Target = pub_sub::Manager<NotificationPayload, (String, Kind), OnSubscription>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl PubSubManager {
    /// Helper function to emit a ProofState status
    pub fn proof_state<E: Into<ProofState>>(&self, event: E) {
        self.broadcast(event.into().into());
    }

    /// Helper function to emit a MintQuoteBolt11Response status
    pub fn mint_quote_bolt11_status<E: Into<MintQuoteBolt11Response>>(
        &self,
        quote: E,
        new_state: MintQuoteState,
    ) {
        let mut event = quote.into();
        event.state = new_state;

        self.broadcast(event.into());
    }

    /// Helper function to emit a MeltQuoteBolt11Response status
    pub fn melt_quote_status<E: Into<MeltQuoteBolt11Response>>(
        &self,
        quote: E,
        payment_preimage: Option<String>,
        change: Option<Vec<BlindSignature>>,
        new_state: MeltQuoteState,
    ) {
        let mut quote = quote.into();
        quote.state = new_state;
        quote.paid = Some(new_state == MeltQuoteState::Paid);
        quote.payment_preimage = payment_preimage;
        quote.change = change;
        self.broadcast(quote.into());
    }
}

#[cfg(test)]
mod test {
    use std::time::Duration;

    use tokio::time::sleep;

    use super::*;
    use crate::nuts::{PublicKey, State};

    #[tokio::test]
    async fn active_and_drop() {
        let manager = PubSubManager::default();
        let params = Params {
            kind: Kind::ProofState,
            filters: vec!["x".to_string()],
            id: "uno".into(),
        };

        // Although the same param is used, two subscriptions are created, that
        // is because each index is unique, thanks to `Unique`, it is the
        // responsibility of the implementor to make sure that SubId are unique
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
        let manager = PubSubManager::default();
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
        let manager = PubSubManager::default();
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

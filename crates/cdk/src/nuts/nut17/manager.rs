//! Specific Subscription for the cdk crate
use std::ops::Deref;
use std::sync::Arc;

use uuid::Uuid;

use super::{Notification, NotificationPayload, OnSubscription};
use crate::cdk_database::{self, MintDatabase};
use crate::nuts::{
    BlindSignature, MeltQuoteBolt11Response, MeltQuoteState, MintQuoteBolt11Response,
    MintQuoteState, ProofState,
};
use crate::pub_sub;

/// Manager
/// Publish–subscribe manager
///
/// Nut-17 implementation is system-wide and not only through the WebSocket, so
/// it is possible for another part of the system to subscribe to events.
pub struct PubSubManager(pub_sub::Manager<NotificationPayload<Uuid>, Notification, OnSubscription>);

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
    type Target = pub_sub::Manager<NotificationPayload<Uuid>, Notification, OnSubscription>;

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
    pub fn mint_quote_bolt11_status<E: Into<MintQuoteBolt11Response<Uuid>>>(
        &self,
        quote: E,
        new_state: MintQuoteState,
    ) {
        let mut event = quote.into();
        event.state = new_state;

        self.broadcast(event.into());
    }

    /// Helper function to emit a MeltQuoteBolt11Response status
    pub fn melt_quote_status<E: Into<MeltQuoteBolt11Response<Uuid>>>(
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
    use crate::nuts::nut17::{Kind, Params};
    use crate::nuts::{PublicKey, State};

    #[tokio::test]
    async fn active_and_drop() {
        let manager = PubSubManager::default();
        let params = Params {
            kind: Kind::ProofState,
            filters: vec![
                "02a9acc1e48c25eeeb9289b5031cc57da9fe72f3fe2861d264bdc074209b107ba2".to_owned(),
            ],
            id: "uno".into(),
        };

        // Although the same param is used, two subscriptions are created, that
        // is because each index is unique, thanks to `Unique`, it is the
        // responsibility of the implementor to make sure that SubId are unique
        // either globally or per client
        let subscriptions = vec![
            manager
                .try_subscribe(params.clone())
                .await
                .expect("valid subscription"),
            manager
                .try_subscribe(params)
                .await
                .expect("valid subscription"),
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
                .try_subscribe(Params {
                    kind: Kind::ProofState,
                    filters: vec![
                        "02194603ffa36356f4a56b7df9371fc3192472351453ec7398b8da8117e7c3e104"
                            .to_string(),
                    ],
                    id: "uno".into(),
                })
                .await
                .expect("valid subscription"),
            manager
                .try_subscribe(Params {
                    kind: Kind::ProofState,
                    filters: vec![
                        "02194603ffa36356f4a56b7df9371fc3192472351453ec7398b8da8117e7c3e104"
                            .to_string(),
                    ],
                    id: "dos".into(),
                })
                .await
                .expect("valid subscription"),
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
            .try_subscribe::<Params>(
                serde_json::from_str(r#"{"kind":"proof_state","filters":["02194603ffa36356f4a56b7df9371fc3192472351453ec7398b8da8117e7c3e104"],"subId":"uno"}"#)
                    .expect("valid json"),
            )
            .await.expect("valid subscription");

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

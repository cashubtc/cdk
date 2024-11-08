use super::{
    handler::{WsHandle, WsNotification},
    WsContext, WsError, JSON_RPC_VERSION,
};
use cdk::{
    nuts::{
        nut17::{Kind, NotificationPayload, Params},
        MeltQuoteBolt11Response, MintQuoteBolt11Response, ProofState, PublicKey,
    },
    pub_sub::SubId,
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Method(Params);

#[derive(Debug, Clone, serde::Serialize)]
/// The response to a subscription request
pub struct Response {
    /// Status
    status: String,
    /// Subscription ID
    #[serde(rename = "subId")]
    sub_id: SubId,
}

#[derive(Debug, Clone, serde::Serialize)]
/// The notification
///
/// This is the notification that is sent to the client when an event matches a
/// subscription
pub struct Notification {
    /// The subscription ID
    #[serde(rename = "subId")]
    pub sub_id: SubId,

    /// The notification payload
    pub payload: NotificationPayload,
}

impl From<(SubId, NotificationPayload)> for WsNotification<Notification> {
    fn from((sub_id, payload): (SubId, NotificationPayload)) -> Self {
        WsNotification {
            jsonrpc: JSON_RPC_VERSION.to_owned(),
            method: "subscribe".to_string(),
            params: Notification { sub_id, payload },
        }
    }
}

#[async_trait::async_trait]
impl WsHandle for Method {
    type Response = Response;

    /// The `handle` method is called when a client sends a subscription request
    async fn handle(self, context: &mut WsContext) -> Result<Self::Response, WsError> {
        let sub_id = self.0.id.clone();
        if context.subscriptions.contains_key(&sub_id) {
            // Subscription ID already exits. Returns an error instead of
            // replacing the other subscription or avoiding it.
            return Err(WsError::InvalidParams);
        }

        let mut subscription = context
            .state
            .mint
            .pubsub_manager
            .subscribe(self.0.clone())
            .await;
        let publisher = context.publisher.clone();

        let current_notification_to_send: Vec<NotificationPayload> = match self.0.kind {
            Kind::Bolt11MeltQuote => {
                let queries = self
                    .0
                    .filters
                    .iter()
                    .map(|id| context.state.mint.localstore.get_melt_quote(id))
                    .collect::<Vec<_>>();

                futures::future::try_join_all(queries)
                    .await
                    .map(|quotes| {
                        quotes
                            .into_iter()
                            .filter_map(|quote| quote.map(|x| x.into()))
                            .map(|x: MeltQuoteBolt11Response| x.into())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            }
            Kind::Bolt11MintQuote => {
                let queries = self
                    .0
                    .filters
                    .iter()
                    .map(|id| context.state.mint.localstore.get_mint_quote(id))
                    .collect::<Vec<_>>();

                futures::future::try_join_all(queries)
                    .await
                    .map(|quotes| {
                        quotes
                            .into_iter()
                            .filter_map(|quote| quote.map(|x| x.into()))
                            .map(|x: MintQuoteBolt11Response| x.into())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            }
            Kind::ProofState => {
                if let Ok(public_keys) = self
                    .0
                    .filters
                    .iter()
                    .map(PublicKey::from_hex)
                    .collect::<Result<Vec<PublicKey>, _>>()
                {
                    context
                        .state
                        .mint
                        .localstore
                        .get_proofs_states(&public_keys)
                        .await
                        .map(|x| {
                            x.into_iter()
                                .enumerate()
                                .filter_map(|(idx, state)| {
                                    state.map(|state| (public_keys[idx], state).into())
                                })
                                .map(|x: ProofState| x.into())
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                } else {
                    vec![]
                }
            }
        };

        for notification in current_notification_to_send.into_iter() {
            let _ = publisher.send((sub_id.clone(), notification)).await;
        }

        context.subscriptions.insert(
            sub_id.clone(),
            tokio::spawn(async move {
                while let Some(response) = subscription.recv().await {
                    let _ = publisher.send(response).await;
                }
            }),
        );
        Ok(Response {
            status: "OK".to_string(),
            sub_id,
        })
    }
}

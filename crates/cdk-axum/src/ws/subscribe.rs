use cdk::nuts::nut17::{NotificationPayload, Params};
use cdk::pub_sub::SubId;

use super::handler::{WsHandle, WsNotification};
use super::{WsContext, WsError, JSON_RPC_VERSION};

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

        let mut subscription = context.state.mint.pubsub_manager.subscribe(self.0).await;
        let publisher = context.publisher.clone();
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

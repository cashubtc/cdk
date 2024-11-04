use super::{
    handler::{WsHandle, WsNotification},
    WsContext, WsError, JSON_RPC_VERSION,
};
use cdk::{
    nuts::nut17::{NotificationPayload, Params},
    pub_sub::SubId,
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Method(Params);

#[derive(Debug, Clone, serde::Serialize)]
pub struct Response {
    status: String,
    #[serde(rename = "subId")]  
    sub_id: SubId,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Notification {
    #[serde(rename = "subId")]
    pub sub_id: SubId,

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

    async fn handle(self, context: &mut WsContext) -> Result<Self::Response, WsError> {
        let sub_id = self.0.id.clone();
        if context.subscriptions.contains_key(&sub_id) {
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

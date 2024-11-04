use super::{handler::WsHandle, WsContext, WsError};
use cdk::pub_sub::SubId;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Method {
    #[serde(rename = "subId")]
    pub sub_id: SubId,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Response {
    status: String,
    sub_id: SubId,
}

#[async_trait::async_trait]
impl WsHandle for Method {
    type Response = Response;

    async fn handle(self, context: &mut WsContext) -> Result<Self::Response, WsError> {
        if context.subscriptions.remove(&self.sub_id).is_some() {
            Ok(Response {
                status: "OK".to_string(),
                sub_id: self.sub_id,
            })
        } else {
            Err(WsError::InvalidParams)
        }
    }
}

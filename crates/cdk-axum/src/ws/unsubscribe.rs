use cdk::nuts::nut17::ws::{WsResponseResult, WsUnsubscribeRequest, WsUnsubscribeResponse};

use super::{WsContext, WsError};

pub(crate) async fn handle(
    context: &mut WsContext,
    req: WsUnsubscribeRequest,
) -> Result<WsResponseResult, WsError> {
    if context.subscriptions.remove(&req.sub_id).is_some() {
        Ok(WsUnsubscribeResponse {
            status: "OK".to_string(),
            sub_id: req.sub_id,
        }
        .into())
    } else {
        Err(WsError::InvalidParams)
    }
}

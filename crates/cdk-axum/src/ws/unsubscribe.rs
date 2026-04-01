use cdk::ws::{WsResponseResult, WsUnsubscribeRequest, WsUnsubscribeResponse};

use super::{WsContext, WsError};

pub(crate) async fn handle(
    context: &mut WsContext,
    req: WsUnsubscribeRequest,
) -> Result<WsResponseResult, WsError> {
    if let Some(handle) = context.subscriptions.remove(&req.sub_id) {
        handle.abort();
        Ok(WsUnsubscribeResponse {
            status: "OK".to_string(),
            sub_id: req.sub_id,
        }
        .into())
    } else {
        Err(WsError::InvalidParams)
    }
}

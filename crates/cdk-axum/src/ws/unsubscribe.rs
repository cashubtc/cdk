use cdk::ws::{WsResponseResult, WsUnsubscribeRequest};

use super::{WsContext, WsError};

pub(crate) async fn handle(
    context: &mut WsContext,
    req: WsUnsubscribeRequest,
) -> Result<WsResponseResult, WsError> {
    if let Some(handle) = context.subscriptions.remove(&req.sub_id) {
        handle.abort();
        Ok(WsResponseResult {
            status: "OK".to_string(),
            sub_id: req.sub_id,
        })
    } else {
        Err(WsError::InvalidParams)
    }
}

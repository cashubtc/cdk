use cdk::ws::{WsResponseResult, WsUnsubscribeRequest};

use super::{WsContext, WsError};

pub(crate) async fn handle(
    context: &mut WsContext,
    req: WsUnsubscribeRequest,
) -> Result<WsResponseResult, WsError> {
    if let Some(slot) = context.subscriptions.remove(&req.sub_id) {
        slot.handle.abort();
        context.topics_in_use = context.topics_in_use.saturating_sub(slot.topics);
        Ok(WsResponseResult {
            status: "OK".to_string(),
            sub_id: req.sub_id,
        })
    } else {
        Err(WsError::InvalidParams)
    }
}

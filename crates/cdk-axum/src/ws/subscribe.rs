use cdk::nuts::nut17::ws::{WsResponseResult, WsSubscribeResponse};
use cdk::nuts::nut17::Params;

use super::{WsContext, WsError};

/// The `handle` method is called when a client sends a subscription request
pub(crate) async fn handle(
    context: &mut WsContext,
    params: Params,
) -> Result<WsResponseResult, WsError> {
    let sub_id = params.id.clone();
    if context.subscriptions.contains_key(&sub_id) {
        // Subscription ID already exits. Returns an error instead of
        // replacing the other subscription or avoiding it.
        return Err(WsError::InvalidParams);
    }

    let mut subscription = context
        .state
        .mint
        .pubsub_manager
        .try_subscribe(params)
        .await
        .map_err(|_| WsError::ParseError)?;

    let publisher = context.publisher.clone();
    context.subscriptions.insert(
        sub_id.clone(),
        tokio::spawn(async move {
            while let Some(response) = subscription.recv().await {
                let _ = publisher.send(response).await;
            }
        }),
    );
    Ok(WsSubscribeResponse {
        status: "OK".to_string(),
        sub_id,
    }
    .into())
}

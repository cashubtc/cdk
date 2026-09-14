use cdk::subscription::Params;
use cdk::ws::WsResponseResult;
use cdk_common::pub_sub::Error as PubSubError;

use super::{SubscriptionSlot, WsContext, WsError};

/// The `handle` method is called when a client sends a subscription request.
///
/// The read loop has already charged the connection's request budget for this
/// frame and its filters, so everything below is about what the mint can admit,
/// not about what the client can afford.
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

    let limits = context.state.ws_limiter.limits();

    if context.subscriptions.len() >= limits.max_subscriptions_per_connection {
        tracing::warn!(
            "WebSocket subscription request exceeds per-connection limit: {} >= {}",
            context.subscriptions.len(),
            limits.max_subscriptions_per_connection
        );
        return Err(WsError::ServerBusy);
    }

    if params.filters.len() > limits.max_filters_per_subscription {
        tracing::warn!(
            "WebSocket subscription request exceeds max filters limit: {} > {}",
            params.filters.len(),
            limits.max_filters_per_subscription
        );
        return Err(WsError::InvalidParams);
    }

    // Each filter registers one topic, so the filter count is what this
    // subscription will claim from the connection's topic budget.
    let requested = params.filters.len();
    let topics_in_use = context.topics_in_use.saturating_add(requested);
    if topics_in_use > limits.max_topics_per_connection {
        tracing::warn!(
            "WebSocket subscription request exceeds per-connection topic budget: {} > {}",
            topics_in_use,
            limits.max_topics_per_connection
        );
        return Err(WsError::ServerBusy);
    }

    let mut subscription = context
        .state
        .mint
        .pubsub_manager()
        .subscribe(params)
        .map_err(|err| match err {
            PubSubError::ParsingError(_) => WsError::InvalidParams,
            PubSubError::TooManyTopics => {
                tracing::warn!("Mint-wide subscription topic budget exhausted: {err}");
                WsError::ServerBusy
            }
            err => {
                tracing::warn!("Could not create subscription: {err}");
                WsError::InternalError
            }
        })?;

    let publisher = context.publisher.clone();
    let delivery_failed = context.delivery_failed.clone();
    let delivery_timeout = limits.idle_timeout;
    let sub_id_for_sender = sub_id.clone();
    context.subscriptions.insert(
        sub_id.clone(),
        SubscriptionSlot {
            handle: tokio::spawn(async move {
                while let Some(response) = subscription.recv().await {
                    let event = (sub_id_for_sender.clone(), response.into_inner());
                    let Err(err) = publisher.send_timeout(event, delivery_timeout).await else {
                        continue;
                    };

                    tracing::debug!("Could not deliver a notification, closing: {err}");
                    if let Err(err) = delivery_failed.try_send(()) {
                        tracing::debug!(
                            "Delivery failure not signalled, the connection is already closing: {err}"
                        );
                    }
                    return;
                }
            }),
            topics: requested,
        },
    );
    context.topics_in_use = topics_in_use;

    Ok(WsResponseResult {
        status: "OK".to_string(),
        sub_id,
    })
}

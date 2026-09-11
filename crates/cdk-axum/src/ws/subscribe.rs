use std::time::Instant;

use cdk::subscription::Params;
use cdk::ws::WsResponseResult;
use cdk_common::pub_sub::Error as PubSubError;

use super::{Charge, SubscriptionSlot, WsContext, WsError};

/// The `handle` method is called when a client sends a subscription request.
///
/// The request is charged to the connection's budget per filter, because
/// registering a filter is what takes the mint-wide topic lock. A flat charge
/// would leave subscribe and unsubscribe churn as cheap as any other frame.
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

    let units = u32::try_from(requested)
        .unwrap_or(u32::MAX)
        .saturating_add(1);
    if context.budget.charge(units, Instant::now()) != Charge::Accepted {
        tracing::debug!("WebSocket subscription request exceeds the connection's request budget");
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
    let sub_id_for_sender = sub_id.clone();
    context.subscriptions.insert(
        sub_id.clone(),
        SubscriptionSlot {
            handle: tokio::spawn(async move {
                while let Some(response) = subscription.recv().await {
                    // Dropped rather than awaited on purpose: blocking here would
                    // let one slow socket stall every other subscriber sharing
                    // this connection's writer.
                    if let Err(err) =
                        publisher.try_send((sub_id_for_sender.clone(), response.into_inner()))
                    {
                        tracing::debug!("Dropping notification for a slow connection: {err}");
                    }
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

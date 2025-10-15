//! Websocket types and functions for the CDK.
//!
//! This module extends the `cashu` crate with types and functions for the CDK, using the correct
//! expected ID types.
use std::sync::Arc;

#[cfg(feature = "mint")]
use cashu::nut17::ws::JSON_RPC_VERSION;
use cashu::nut17::{self};
#[cfg(feature = "mint")]
use cashu::quote_id::QuoteId;
#[cfg(feature = "mint")]
use cashu::NotificationPayload;

type SubId = Arc<crate::subscription::SubId>;

/// Request to unsubscribe from a websocket subscription
pub type WsUnsubscribeRequest = nut17::ws::WsUnsubscribeRequest<SubId>;

/// Notification message sent over websocket
pub type WsNotification = nut17::ws::WsNotification<SubId>;

/// Response to a subscription request
pub type WsSubscribeResponse = nut17::ws::WsSubscribeResponse<SubId>;

/// Result part of a websocket response
pub type WsResponseResult = nut17::ws::WsResponseResult<SubId>;

/// Response to an unsubscribe request
pub type WsUnsubscribeResponse = nut17::ws::WsUnsubscribeResponse<SubId>;

/// Generic websocket request
pub type WsRequest = nut17::ws::WsRequest<SubId>;

/// Generic websocket response
pub type WsResponse = nut17::ws::WsResponse<SubId>;

/// Method-specific websocket request
pub type WsMethodRequest = nut17::ws::WsMethodRequest<SubId>;

/// Error body for websocket responses
pub type WsErrorBody = nut17::ws::WsErrorBody;

/// Either a websocket message or a response
pub type WsMessageOrResponse = nut17::ws::WsMessageOrResponse<SubId>;

/// Inner content of a notification with generic payload type
pub type NotificationInner<T> = nut17::ws::NotificationInner<T, SubId>;

#[cfg(feature = "mint")]
/// Converts a notification with UUID identifiers to a notification with string identifiers
pub fn notification_uuid_to_notification_string(
    notification: NotificationInner<QuoteId>,
) -> NotificationInner<String> {
    nut17::ws::NotificationInner {
        sub_id: notification.sub_id,
        payload: match notification.payload {
            NotificationPayload::ProofState(pk) => NotificationPayload::ProofState(pk),
            NotificationPayload::MeltQuoteBolt11Response(quote) => {
                NotificationPayload::MeltQuoteBolt11Response(quote.to_string_id())
            }
            NotificationPayload::MintQuoteBolt11Response(quote) => {
                NotificationPayload::MintQuoteBolt11Response(quote.to_string_id())
            }
            NotificationPayload::MintQuoteBolt12Response(quote) => {
                NotificationPayload::MintQuoteBolt12Response(quote.to_string_id())
            }
            NotificationPayload::MintQuoteMiningShareResponse(quote) => {
                NotificationPayload::MintQuoteMiningShareResponse(quote.to_string_id())
            }
        },
    }
}

#[cfg(feature = "mint")]
/// Converts a notification to a websocket message that can be sent to clients
pub fn notification_to_ws_message(notification: NotificationInner<QuoteId>) -> WsMessageOrResponse {
    nut17::ws::WsMessageOrResponse::Notification(nut17::ws::WsNotification {
        jsonrpc: JSON_RPC_VERSION.to_owned(),
        method: "subscribe".to_string(),
        params: notification_uuid_to_notification_string(notification),
    })
}

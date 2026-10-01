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

/// Result part of a websocket response
pub type WsResponseResult = nut17::ws::WsResponseResult<SubId>;

/// Result of a subscribe or unsubscribe request
pub type WsSubscriptionResult = nut17::ws::WsSubscriptionResult<SubId>;

/// Generic websocket request
pub type WsRequest = nut17::ws::WsRequest<SubId>;

/// Generic websocket response
pub type WsResponse = nut17::ws::WsResponse<SubId>;

/// Method-specific websocket request
pub type WsMethodRequest = nut17::ws::WsMethodRequest<SubId>;

/// Request to authenticate a connection (NUT-22)
pub use nut17::ws::WsAuthenticateRequest;
/// Response to an authenticate request (NUT-22)
pub use nut17::ws::WsAuthenticateResponse;

/// Error body for websocket responses
pub type WsErrorBody = nut17::ws::WsErrorBody;

/// Either a websocket message or a response
pub type WsMessageOrResponse = nut17::ws::WsMessageOrResponse<SubId>;

/// Raw notification content with an undecoded JSON payload
pub type RawNotificationInner = nut17::ws::RawNotificationInner<SubId>;

/// Either a websocket message or a response with raw notification payloads
pub type RawWsMessageOrResponse = nut17::ws::RawWsMessageOrResponse<SubId>;

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
            NotificationPayload::MeltQuoteBolt12Response(quote) => {
                NotificationPayload::MeltQuoteBolt12Response(quote.to_string_id())
            }
            NotificationPayload::CustomMintQuoteResponse(method, quote) => {
                NotificationPayload::CustomMintQuoteResponse(method, quote.to_string_id())
            }
            NotificationPayload::CustomMeltQuoteResponse(method, quote) => {
                NotificationPayload::CustomMeltQuoteResponse(method, quote.to_string_id())
            }
            NotificationPayload::MeltQuoteOnchainResponse(quote) => {
                NotificationPayload::MeltQuoteOnchainResponse(quote.to_string_id())
            }
            NotificationPayload::MintQuoteOnchainResponse(quote) => {
                NotificationPayload::MintQuoteOnchainResponse(quote.to_string_id())
            }
        },
    }
}

#[cfg(feature = "mint")]
/// Converts a notification to a websocket message that can be sent to clients
pub fn notification_to_ws_message(notification: NotificationInner<QuoteId>) -> WsMessageOrResponse {
    nut17::ws::WsMessageOrResponse::Notification(Box::new(nut17::ws::WsNotification {
        jsonrpc: JSON_RPC_VERSION.to_owned(),
        method: "subscribe".to_string(),
        params: notification_uuid_to_notification_string(notification),
    }))
}

#[cfg(test)]
mod tests {
    use cashu::nut17::MAX_SUBSCRIPTION_ID_LEN;
    use serde_json::json;

    use super::*;

    #[test]
    fn websocket_requests_reject_oversized_subscription_ids() {
        let oversized_id = "a".repeat(MAX_SUBSCRIPTION_ID_LEN + 1);
        let subscribe = json!({
            "jsonrpc": "2.0",
            "method": "subscribe",
            "params": {
                "kind": "bolt11_mint_quote",
                "filters": [],
                "subId": oversized_id,
            },
            "id": 1,
        });
        assert!(serde_json::from_value::<WsRequest>(subscribe).is_err());

        let unsubscribe = json!({
            "jsonrpc": "2.0",
            "method": "unsubscribe",
            "params": {
                "subId": "a".repeat(MAX_SUBSCRIPTION_ID_LEN + 1),
            },
            "id": 2,
        });
        assert!(serde_json::from_value::<WsRequest>(unsubscribe).is_err());
    }
}

//! Websocket types and functions for the CDK.
//!
//! This module extends the `cashu` crate with types and functions for the CDK, using the correct
//! expected ID types.
#[cfg(feature = "mint")]
use cashu::nut17::ws::JSON_RPC_VERSION;
use cashu::nut17::{self};
#[cfg(feature = "mint")]
use cashu::NotificationPayload;
#[cfg(feature = "mint")]
use uuid::Uuid;

use crate::pub_sub::SubId;

pub type WsUnsubscribeRequest = nut17::ws::WsUnsubscribeRequest<SubId>;
pub type WsNotification = nut17::ws::WsNotification<SubId>;
pub type WsSubscribeResponse = nut17::ws::WsSubscribeResponse<SubId>;
pub type WsResponseResult = nut17::ws::WsResponseResult<SubId>;
pub type WsUnsubscribeResponse = nut17::ws::WsUnsubscribeResponse<SubId>;
pub type WsRequest = nut17::ws::WsRequest<SubId>;
pub type WsResponse = nut17::ws::WsResponse<SubId>;
pub type WsMethodRequest = nut17::ws::WsMethodRequest<SubId>;
pub type WsErrorBody = nut17::ws::WsErrorBody;
pub type WsMessageOrResponse = nut17::ws::WsMessageOrResponse<SubId>;
pub type NotificationInner<T> = nut17::ws::NotificationInner<T, SubId>;

#[cfg(feature = "mint")]
pub fn notification_uuid_to_notification_string(
    notification: NotificationInner<Uuid>,
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
        },
    }
}

#[cfg(feature = "mint")]
pub fn notification_to_ws_message(notification: NotificationInner<Uuid>) -> WsMessageOrResponse {
    nut17::ws::WsMessageOrResponse::Notification(nut17::ws::WsNotification {
        jsonrpc: JSON_RPC_VERSION.to_owned(),
        method: "subscribe".to_string(),
        params: notification_uuid_to_notification_string(notification),
    })
}

//! Websocket types

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{NotificationPayload, Params, SubId};

/// JSON RPC version
pub const JSON_RPC_VERSION: &str = "2.0";

/// The response to a subscription request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsSubscribeResponse {
    /// Status
    pub status: String,
    /// Subscription ID
    #[serde(rename = "subId")]
    pub sub_id: SubId,
}

/// The response to an unsubscription request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsUnsubscribeResponse {
    /// Status
    pub status: String,
    /// Subscription ID
    #[serde(rename = "subId")]
    pub sub_id: SubId,
}

/// The notification
///
/// This is the notification that is sent to the client when an event matches a
/// subscription
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound = "T: Serialize + DeserializeOwned")]
pub struct NotificationInner<T> {
    /// The subscription ID
    #[serde(rename = "subId")]
    pub sub_id: SubId,

    /// The notification payload
    pub payload: NotificationPayload<T>,
}

impl From<NotificationInner<Uuid>> for NotificationInner<String> {
    fn from(value: NotificationInner<Uuid>) -> Self {
        NotificationInner {
            sub_id: value.sub_id,
            payload: match value.payload {
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
}

/// Responses from the web socket server
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WsResponseResult {
    /// A response to a subscription request
    Subscribe(WsSubscribeResponse),
    /// Unsubscribe
    Unsubscribe(WsUnsubscribeResponse),
}

impl From<WsSubscribeResponse> for WsResponseResult {
    fn from(response: WsSubscribeResponse) -> Self {
        WsResponseResult::Subscribe(response)
    }
}

impl From<WsUnsubscribeResponse> for WsResponseResult {
    fn from(response: WsUnsubscribeResponse) -> Self {
        WsResponseResult::Unsubscribe(response)
    }
}

/// The request to unsubscribe
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsUnsubscribeRequest {
    /// Subscription ID
    #[serde(rename = "subId")]
    pub sub_id: SubId,
}

/// The inner method of the websocket request
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "method", content = "params")]
pub enum WsMethodRequest {
    /// Subscribe method
    Subscribe(Params),
    /// Unsubscribe method
    Unsubscribe(WsUnsubscribeRequest),
}

/// Websocket request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsRequest {
    /// JSON RPC version
    pub jsonrpc: String,
    /// The method body
    #[serde(flatten)]
    pub method: WsMethodRequest,
    /// The request ID
    pub id: usize,
}

impl From<(WsMethodRequest, usize)> for WsRequest {
    fn from((method, id): (WsMethodRequest, usize)) -> Self {
        WsRequest {
            jsonrpc: JSON_RPC_VERSION.to_owned(),
            method,
            id,
        }
    }
}

/// Notification from the server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsNotification<T> {
    /// JSON RPC version
    pub jsonrpc: String,
    /// The method
    pub method: String,
    /// The parameters
    pub params: T,
}

/// Websocket error
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WsErrorBody {
    /// Error code
    pub code: i32,
    /// Error message
    pub message: String,
}

/// Websocket response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsResponse {
    /// JSON RPC version
    pub jsonrpc: String,
    /// The result
    pub result: WsResponseResult,
    /// The request ID
    pub id: usize,
}

/// WebSocket error response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsErrorResponse {
    /// JSON RPC version
    pub jsonrpc: String,
    /// The result
    pub error: WsErrorBody,
    /// The request ID
    pub id: usize,
}

/// Message from the server to the client
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WsMessageOrResponse {
    /// A response to a request
    Response(WsResponse),
    /// An error response
    ErrorResponse(WsErrorResponse),
    /// A notification
    Notification(WsNotification<NotificationInner<String>>),
}

impl From<(usize, Result<WsResponseResult, WsErrorBody>)> for WsMessageOrResponse {
    fn from((id, result): (usize, Result<WsResponseResult, WsErrorBody>)) -> Self {
        match result {
            Ok(result) => WsMessageOrResponse::Response(WsResponse {
                jsonrpc: JSON_RPC_VERSION.to_owned(),
                result,
                id,
            }),
            Err(err) => WsMessageOrResponse::ErrorResponse(WsErrorResponse {
                jsonrpc: JSON_RPC_VERSION.to_owned(),
                error: err,
                id,
            }),
        }
    }
}

impl From<NotificationInner<Uuid>> for WsMessageOrResponse {
    fn from(notification: NotificationInner<Uuid>) -> Self {
        WsMessageOrResponse::Notification(WsNotification {
            jsonrpc: JSON_RPC_VERSION.to_owned(),
            method: "subscribe".to_string(),
            params: notification.into(),
        })
    }
}

impl From<NotificationInner<String>> for WsMessageOrResponse {
    fn from(notification: NotificationInner<String>) -> Self {
        WsMessageOrResponse::Notification(WsNotification {
            jsonrpc: JSON_RPC_VERSION.to_owned(),
            method: "subscribe".to_string(),
            params: notification,
        })
    }
}

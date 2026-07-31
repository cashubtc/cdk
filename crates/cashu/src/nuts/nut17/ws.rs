//! Websocket types

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::{NotificationPayload, Params};

/// JSON RPC version
pub const JSON_RPC_VERSION: &str = "2.0";

/// The response to a subscription request
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound = "I: Serialize + DeserializeOwned")]
pub struct WsSubscribeResponse<I> {
    /// Status
    pub status: String,
    /// Subscription ID
    #[serde(rename = "subId")]
    pub sub_id: I,
}

/// The response to an unsubscription request
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound = "I: Serialize + DeserializeOwned")]
pub struct WsUnsubscribeResponse<I> {
    /// Status
    pub status: String,
    /// Subscription ID
    #[serde(rename = "subId")]
    pub sub_id: I,
}

/// The notification
///
/// This is the notification that is sent to the client when an event matches a
/// subscription
///
/// This type is serialize-only in practice: notification payloads are not
/// self-describing. Clients should deserialize notifications as
/// [`RawNotificationInner`] and decode the payload with
/// [`deserialize_payload_for_kind`](super::deserialize_payload_for_kind), using
/// the kind of the subscription the notification belongs to.
#[derive(Debug, Clone, Serialize)]
#[serde(bound(serialize = "T: Serialize + DeserializeOwned, I: Serialize"))]
pub struct NotificationInner<T, I>
where
    T: Clone,
{
    /// The subscription ID
    #[serde(rename = "subId")]
    pub sub_id: I,

    /// The notification payload
    pub payload: NotificationPayload<T>,
}

/// The raw notification received from the websocket server.
///
/// This keeps the payload as JSON so clients can decode it with the kind of the
/// subscription that produced the notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound = "I: Serialize + DeserializeOwned")]
pub struct RawNotificationInner<I> {
    /// The subscription ID
    #[serde(rename = "subId")]
    pub sub_id: I,

    /// The raw notification payload
    pub payload: serde_json::Value,
}

/// The response to an authenticate request (NUT-22)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "UPPERCASE")]
pub enum WsAuthenticateResponse {
    /// Authentication succeeded
    Ok,
}

/// Responses from the web socket server
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound = "I: Serialize + DeserializeOwned")]
#[serde(untagged)]
pub enum WsResponseResult<I> {
    /// A response to a subscription request
    Subscribe(WsSubscribeResponse<I>),
    /// Unsubscribe
    Unsubscribe(WsUnsubscribeResponse<I>),
    /// A response to an authenticate request
    ///
    /// Declared last so untagged deserialization tries the subscribe and
    /// unsubscribe variants first: both require a `subId`, so a body without
    /// one only matches here.
    Authenticate(WsAuthenticateResponse),
}

impl<I> From<WsSubscribeResponse<I>> for WsResponseResult<I> {
    fn from(response: WsSubscribeResponse<I>) -> Self {
        WsResponseResult::Subscribe(response)
    }
}

impl<I> From<WsUnsubscribeResponse<I>> for WsResponseResult<I> {
    fn from(response: WsUnsubscribeResponse<I>) -> Self {
        WsResponseResult::Unsubscribe(response)
    }
}

impl<I> From<WsAuthenticateResponse> for WsResponseResult<I> {
    fn from(response: WsAuthenticateResponse) -> Self {
        WsResponseResult::Authenticate(response)
    }
}

/// The request to unsubscribe
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound = "I: Serialize + DeserializeOwned")]
pub struct WsUnsubscribeRequest<I> {
    /// Subscription ID
    #[serde(rename = "subId")]
    pub sub_id: I,
}

/// The request to authenticate a connection (NUT-22)
///
/// Carries a blind authentication token (BAT), the serialized `authA...`
/// string, so browser wallets can authenticate a protected connection in-band
/// (the WebSocket API cannot set the `Blind-auth` header).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsAuthenticateRequest {
    /// The blind authentication token
    pub token: String,
}

/// The inner method of the websocket request
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "method", content = "params")]
#[serde(bound = "I: Serialize + DeserializeOwned")]
pub enum WsMethodRequest<I> {
    /// Subscribe method
    Subscribe(Params<I>),
    /// Unsubscribe method
    Unsubscribe(WsUnsubscribeRequest<I>),
    /// Authenticate method (NUT-22)
    Authenticate(WsAuthenticateRequest),
}

/// Websocket request
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound = "I: Serialize + DeserializeOwned")]
pub struct WsRequest<I> {
    /// JSON RPC version
    pub jsonrpc: String,
    /// The method body
    #[serde(flatten)]
    pub method: WsMethodRequest<I>,
    /// The request ID
    pub id: usize,
}

impl<I> From<(WsMethodRequest<I>, usize)> for WsRequest<I> {
    fn from((method, id): (WsMethodRequest<I>, usize)) -> Self {
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
#[serde(bound = "I: Serialize + DeserializeOwned")]
pub struct WsResponse<I> {
    /// JSON RPC version
    pub jsonrpc: String,
    /// The result
    pub result: WsResponseResult<I>,
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
///
/// This type is serialize-only in practice. Clients parsing incoming messages
/// should use [`RawWsMessageOrResponse`] so notification payloads are kept as
/// raw JSON until the subscription kind is known.
#[derive(Debug, Clone, Serialize)]
#[serde(bound(serialize = "I: Serialize + DeserializeOwned"))]
#[serde(untagged)]
pub enum WsMessageOrResponse<I> {
    /// A response to a request
    Response(WsResponse<I>),
    /// An error response
    ErrorResponse(WsErrorResponse),
    /// A notification
    Notification(Box<WsNotification<NotificationInner<String, I>>>),
}

/// Raw message from the server to the client.
///
/// Use this type when deserializing websocket messages from a mint. Notification
/// payloads must then be decoded with
/// [`deserialize_payload_for_kind`](super::deserialize_payload_for_kind).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound = "I: Serialize + DeserializeOwned")]
#[serde(untagged)]
pub enum RawWsMessageOrResponse<I> {
    /// A response to a request
    Response(WsResponse<I>),
    /// An error response
    ErrorResponse(WsErrorResponse),
    /// A notification with raw JSON payload
    Notification(Box<WsNotification<RawNotificationInner<I>>>),
}

impl<I> From<(usize, Result<WsResponseResult<I>, WsErrorBody>)> for WsMessageOrResponse<I> {
    fn from((id, result): (usize, Result<WsResponseResult<I>, WsErrorBody>)) -> Self {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_ws_message_deserializes_notification_payload_as_json() {
        let encoded = r#"{
            "jsonrpc": "2.0",
            "method": "subscribe",
            "params": {
                "subId": "sub-id",
                "payload": {
                    "quote": "quote-id",
                    "method": "bolt12"
                }
            }
        }"#;

        let decoded: RawWsMessageOrResponse<String> =
            serde_json::from_str(encoded).expect("raw websocket notification");

        match decoded {
            RawWsMessageOrResponse::Notification(notification) => {
                assert_eq!(notification.params.sub_id, "sub-id");
                assert_eq!(notification.params.payload["quote"], "quote-id");
            }
            other => panic!("expected notification, got {:?}", other),
        }
    }

    #[test]
    fn authenticate_request_round_trips() {
        let request: WsRequest<String> = (
            WsMethodRequest::Authenticate(WsAuthenticateRequest {
                token: "authAeyJ0ZXN0IjoxfQ".to_string(),
            }),
            0,
        )
            .into();

        let json = serde_json::to_value(&request).expect("serialize authenticate");
        assert_eq!(json["method"], "authenticate");
        assert_eq!(json["params"]["token"], "authAeyJ0ZXN0IjoxfQ");
        assert_eq!(json["id"], 0);

        let decoded: WsRequest<String> =
            serde_json::from_value(json).expect("deserialize authenticate");
        match decoded.method {
            WsMethodRequest::Authenticate(req) => assert_eq!(req.token, "authAeyJ0ZXN0IjoxfQ"),
            other => panic!("expected authenticate, got {:?}", other),
        }
    }

    #[test]
    fn authenticate_response_is_distinct_from_subscribe() {
        // An authenticate OK body has no subId, so untagged decoding must not
        // mistake it for a subscribe/unsubscribe response.
        let decoded: WsResponseResult<String> =
            serde_json::from_str(r#"{"status":"OK"}"#).expect("authenticate response");
        assert!(matches!(decoded, WsResponseResult::Authenticate(_)));

        let decoded: WsResponseResult<String> =
            serde_json::from_str(r#"{"status":"OK","subId":"sub-1"}"#).expect("subscribe response");
        assert!(matches!(decoded, WsResponseResult::Subscribe(_)));
    }

    #[test]
    fn authenticate_response_serializes_with_status_ok() {
        let result: WsResponseResult<String> = WsAuthenticateResponse::Ok.into();
        let json = serde_json::to_value(&result).expect("serialize authenticate response");
        assert_eq!(json, serde_json::json!({ "status": "OK" }));
    }
}

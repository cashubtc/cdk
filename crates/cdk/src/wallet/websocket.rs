//! Websocket types

use serde::{Deserialize, Serialize};

use crate::{nuts::nut17::Params, pub_sub::SubId};

/// JSON RPC version
pub const JSON_RPC_VERSION: &str = "2.0";

/// Websocket request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsRequest {
    pub jsonrpc: String,
    #[serde(flatten)]
    pub method: WsMethod,
    pub id: usize,
}

/// Websocket method
///
/// List of possible methods that can be called on the websocket
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "method", content = "params")]
pub enum WsMethod {
    /// Subscribe to a topic
    Subscribe(Params),
    /// Unsubscribe from a topic
    Unsubscribe(UnsubscribeMethod),
}

/// Unsubscribe method
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UnsubscribeMethod {
    #[serde(rename = "subId")]
    pub sub_id: SubId,
}

/// Websocket error response
#[derive(Debug, Clone, Serialize)]
pub struct WsErrorResponse {
    code: i32,
    message: String,
}

/// Websocket response
#[derive(Debug, Clone, Serialize)]
pub struct WsResponse<T: Serialize + Sized> {
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<WsErrorResponse>,
    id: usize,
}

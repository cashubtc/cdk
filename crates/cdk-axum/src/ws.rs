use axum::extract::ws::{Message, WebSocket};
use cdk::nuts::nut17::{Params, SubId};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::mpsc;

use crate::MintState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsRequest {
    jsonrpc: String,
    #[serde(flatten)]
    method: WsMethod,
    id: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Unsubscribe {
    pub sub_id: SubId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum WsMethod {
    Subscribe(Params),
    Unsubscribe(Unsubscribe),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsSubscribeResponse {
    status: Status,
    #[serde(rename = "subId")]
    sub_id: SubId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Status {
    #[serde(rename = "OK")]
    Ok,
    Err(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Source: https://www.jsonrpc.org/specification#error_object
pub enum WsError {
    /// Invalid JSON was received by the server.
    /// An error occurred on the server while parsing the JSON text.
    ParseError,
    /// The JSON sent is not a valid Request object.
    InvalidRequest,
    /// The method does not exist / is not available.
    MethodNotFound,
    /// Invalid method parameter(s).
    InvalidParams,
    /// Internal JSON-RPC error.
    InternalError,
    /// Custom error
    ServerError(i32, String),
}

impl Into<WsErrorResponse> for WsError {
    fn into(self) -> WsErrorResponse {
        let (id, message) = match self {
            WsError::ParseError => (-32700, "Parse error".to_string()),
            WsError::InvalidRequest => (-32600, "Invalid Request".to_string()),
            WsError::MethodNotFound => (-32601, "Method not found".to_string()),
            WsError::InvalidParams => (-32602, "Invalid params".to_string()),
            WsError::InternalError => (-32603, "Internal error".to_string()),
            WsError::ServerError(code, message) => (code, message),
        };
        WsErrorResponse { code: id, message }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsErrorResponse {
    code: i32,
    message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsResponse<T: Serialize + Sized> {
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<WsErrorResponse>,
    id: usize,
}

impl From<(WsError, usize)> for WsResponse<()> {
    fn from((error, id): (WsError, usize)) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: None,
            error: Some(error.into()),
            id,
        }
    }
}

impl<T: Serialize + Sized> From<(T, usize)> for WsResponse<T> {
    fn from((result, id): (T, usize)) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: Some(result),
            error: None,
            id,
        }
    }
}

async fn reply_to<T: Sized + Serialize>(
    socket: &mut WebSocket,
    response: impl Into<WsResponse<T>> + Serialize,
) {
    let _ = socket
        .send(Message::Text(serde_json::to_string(&response).unwrap()))
        .await;
}

/// Main function for websocket connections
///
/// This function will handle all incoming websocket connections and keep them in their own loop.
///
/// For simplicity sake this function will spawn tasks for each subscription and
/// keep them in a hashmap, and will have a single subscriber for all of them.
pub async fn main_websocket(mut socket: WebSocket, state: MintState) {
    let mut subscriptions = HashMap::new();
    let (publisher, mut subscriber) = mpsc::channel(100);

    loop {
        tokio::select! {
            Some((sub_id, _payload)) = subscriber.recv() => {
                if !subscriptions.contains_key(&sub_id) {
                    // It may be possible an incoming message has come from a dropped Subscriptions that has not yet been
                    // unsubscribed from the subscription manager, just ignore it.
                    continue;
                }
                todo!()
            },
            Some(Ok(Message::Text(text))) = socket.next() => {
                let request = match serde_json::from_str::<WsRequest>(&text) {
                    Ok(request) => request,
                    Err(err) => {
                        tracing::error!("Could not parse request: {}", err);
                        continue;
                    }
                };

                match request.method {
                    WsMethod::Subscribe(params) => {
                        let sub_id = params.id.clone();
                        if subscriptions.contains_key(&sub_id) {
                            reply_to::<()>(&mut socket, (WsError::InvalidParams, request.id)).await;
                            continue;
                        }
                        let mut subscription = state
                            .subscription_manager
                            .subscribe(params)
                            .await;
                        let publisher = publisher.clone();
                        subscriptions.insert(sub_id, tokio::spawn(async move {
                            while let Some(response) = subscription.recv().await {
                                let _ = publisher.send(response).await;
                            }
                        }));

                    }
                    WsMethod::Unsubscribe(unsubscribe) => {
                        // Check the output
                        //
                        // When removing a subscription it goes out of scope and
                        // the subscription manager will trigger the
                        // desubsription
                        let _ = subscriptions.remove(&unsubscribe.sub_id);
                    }
                }
                println!("Received: {}", text);

                if socket.send(Message::Text(text)).await.is_err() {
                    break;
                }
            }
        }
    }
}

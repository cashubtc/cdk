use std::collections::HashMap;

use axum::extract::ws::{Message, WebSocket};
use cdk::nuts::nut17::{NotificationPayload, SubId};
use futures::StreamExt;
use handler::{WsHandle, WsNotification};
use serde::{Deserialize, Serialize};
use subscribe::Notification;
use tokio::sync::mpsc;

use crate::MintState;

mod error;
mod handler;
mod subscribe;
mod unsubscribe;

/// JSON RPC version
pub const JSON_RPC_VERSION: &str = "2.0";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsRequest {
    jsonrpc: String,
    #[serde(flatten)]
    method: WsMethod,
    id: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "method", content = "params")]
pub enum WsMethod {
    Subscribe(subscribe::Method),
    Unsubscribe(unsubscribe::Method),
}

impl WsMethod {
    pub async fn process(
        self,
        req_id: usize,
        context: &mut WsContext,
    ) -> Result<serde_json::Value, serde_json::Error> {
        match self {
            WsMethod::Subscribe(sub) => sub.process(req_id, context),
            WsMethod::Unsubscribe(unsub) => unsub.process(req_id, context),
        }
        .await
    }
}

pub use error::WsError;

pub struct WsContext {
    state: MintState,
    subscriptions: HashMap<SubId, tokio::task::JoinHandle<()>>,
    publisher: mpsc::Sender<(SubId, NotificationPayload)>,
}

/// Main function for websocket connections
///
/// This function will handle all incoming websocket connections and keep them in their own loop.
///
/// For simplicity sake this function will spawn tasks for each subscription and
/// keep them in a hashmap, and will have a single subscriber for all of them.
#[allow(clippy::incompatible_msrv)]
pub async fn main_websocket(mut socket: WebSocket, state: MintState) {
    let (publisher, mut subscriber) = mpsc::channel(100);
    let mut context = WsContext {
        state,
        subscriptions: HashMap::new(),
        publisher,
    };

    loop {
        tokio::select! {
            Some((sub_id, payload)) = subscriber.recv() => {
                if !context.subscriptions.contains_key(&sub_id) {
                    // It may be possible an incoming message has come from a dropped Subscriptions that has not yet been
                    // unsubscribed from the subscription manager, just ignore it.
                    continue;
                }
                let notification: WsNotification<Notification> = (sub_id, payload).into();
                let message = match serde_json::to_string(&notification) {
                    Ok(message) => message,
                    Err(err) => {
                        tracing::error!("Could not serialize notification: {}", err);
                        continue;
                    }
                };

          if let Err(err)= socket.send(Message::Text(message)).await {
                tracing::error!("Could not send websocket message: {}", err);
                break;
          }
            }
            Some(Ok(Message::Text(text))) = socket.next() => {
                let request = match serde_json::from_str::<WsRequest>(&text) {
                    Ok(request) => request,
                    Err(err) => {
                        tracing::error!("Could not parse request: {}", err);
                        continue;
                    }
                };

                match request.method.process(request.id, &mut context).await {
                    Ok(result) => {
                        if let Err(err) = socket
                            .send(Message::Text(result.to_string()))
                            .await
                        {
                            tracing::error!("Could not send request: {}", err);
                            break;
                        }
                    }
                    Err(err) => {
                        tracing::error!("Error serializing response: {}", err);
                        break;
                    }
                }
            }
            else =>  {

            }
        }
    }
}

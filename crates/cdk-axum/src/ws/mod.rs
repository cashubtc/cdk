use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use cdk::mint::QuoteId;
use cdk::nuts::nut17::NotificationPayload;
use cdk::subscription::SubId;
use cdk::ws::{
    notification_to_ws_message, NotificationInner, WsErrorBody, WsMessageOrResponse,
    WsMethodRequest, WsRequest,
};
use futures::StreamExt;
use tokio::sync::mpsc;

use crate::MintState;

mod error;
mod subscribe;
mod unsubscribe;

async fn process(
    context: &mut WsContext,
    body: WsRequest,
) -> Result<serde_json::Value, serde_json::Error> {
    let response = match body.method {
        WsMethodRequest::Subscribe(sub) => subscribe::handle(context, sub).await,
        WsMethodRequest::Unsubscribe(unsub) => unsubscribe::handle(context, unsub).await,
    }
    .map_err(WsErrorBody::from);

    let response: WsMessageOrResponse = (body.id, response).into();

    serde_json::to_value(response)
}

pub use error::WsError;

pub struct WsContext {
    state: MintState,
    subscriptions: HashMap<Arc<SubId>, tokio::task::JoinHandle<()>>,
    publisher: mpsc::Sender<(Arc<SubId>, NotificationPayload<QuoteId>)>,
}

/// Main function for websocket connections
///
/// This function will handle all incoming websocket connections and keep them in their own loop.
///
/// For simplicity sake this function will spawn tasks for each subscription and
/// keep them in a hashmap, and will have a single subscriber for all of them.
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
                let notification = notification_to_ws_message(NotificationInner {
                    sub_id,
                    payload,
                });
                let message = match serde_json::to_string(&notification) {
                    Ok(message) => message,
                    Err(err) => {
                        tracing::error!("Could not serialize notification: {}", err);
                        continue;
                    }
                };

                if let Err(err)= socket.send(Message::Text(message.into())).await {
                    tracing::error!("Could not send websocket message: {}", err);
                    break;
                }
            }

            Some(from_ws) = socket.next() => {
                let text = match from_ws {
                    Ok(Message::Text(text)) => text.to_string(),
                    Ok(Message::Binary(bin)) => String::from_utf8_lossy(&bin).to_string(),
                    Ok(Message::Ping(payload)) => {
                        // Reply with Pong with same payload
                        if let Err(e) = socket.send(Message::Pong(payload)).await {
                            tracing::error!("failed to send pong: {e}");
                            break;
                        }
                        continue;
                    },
                    Ok(Message::Pong(_payload)) => {
                        tracing::error!("Unexpected pong");
                        continue;
                    },
                    Ok(Message::Close(frame)) => {
                        if let Some(CloseFrame { code, reason }) = frame {
                            tracing::info!("ws-close: code={code:?} reason='{reason}'");
                        } else {
                            tracing::info!("ws-close: no frame");
                        }

                        let _ = socket.send(Message::Close(Some(CloseFrame {
                            code: axum::extract::ws::close_code::NORMAL,
                            reason: "bye!".into(),
                        }))).await;
                        break;
                    }
                    Err(err) => {
                        tracing::error!("ws-error: {err}");
                        break;
                    }
                };


                let request = match serde_json::from_str::<WsRequest>(&text) {
                    Ok(request) => request,
                    Err(err) => {
                        tracing::error!("Could not parse request: {}", err);
                        continue;
                    }
                };

                match process(&mut context, request).await {
                    Ok(result) => {
                        if let Err(err) = socket
                            .send(Message::Text(result.to_string().into()))
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
                // Unexpected, we should exit the loop
                tracing::warn!("Unexpected event, closing ws");
                break;
            }
        }
    }
}

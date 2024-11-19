use std::collections::HashMap;

use axum::extract::ws::{Message, WebSocket};
use cdk::nuts::nut17::ws::{
    NotificationInner, WsErrorBody, WsMessageOrResponse, WsMethodRequest, WsRequest,
};
use cdk::nuts::nut17::{NotificationPayload, SubId};
use futures::StreamExt;
use tokio::sync::mpsc;
use uuid::Uuid;

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
    subscriptions: HashMap<SubId, tokio::task::JoinHandle<()>>,
    publisher: mpsc::Sender<(SubId, NotificationPayload<Uuid>)>,
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
                let notification: WsMessageOrResponse= NotificationInner {
                    sub_id,
                    payload,
                }.into();
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

                match process(&mut context, request).await {
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

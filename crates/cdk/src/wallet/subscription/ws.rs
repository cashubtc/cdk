use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use super::http::http_main;
use super::WsSubscriptionBody;
use crate::mint_url::MintUrl;
use crate::nuts::nut17::ws::{
    WsMessageOrResponse, WsMethodRequest, WsRequest, WsUnsubscribeRequest,
};
use crate::nuts::nut17::Params;
use crate::pub_sub::SubId;
use crate::wallet::client::MintConnector;

const MAX_ATTEMPT_FALLBACK_HTTP: usize = 10;

async fn fallback_to_http<S: IntoIterator<Item = SubId>>(
    initial_state: S,
    http_client: Arc<dyn MintConnector + Send + Sync>,
    subscriptions: Arc<RwLock<HashMap<SubId, WsSubscriptionBody>>>,
    new_subscription_recv: mpsc::Receiver<SubId>,
    on_drop: mpsc::Receiver<SubId>,
) {
    http_main(
        initial_state,
        http_client,
        subscriptions,
        new_subscription_recv,
        on_drop,
    )
    .await
}

#[allow(clippy::incompatible_msrv)]
#[inline]
pub async fn ws_main(
    http_client: Arc<dyn MintConnector + Send + Sync>,
    mint_url: MintUrl,
    subscriptions: Arc<RwLock<HashMap<SubId, WsSubscriptionBody>>>,
    mut new_subscription_recv: mpsc::Receiver<SubId>,
    mut on_drop: mpsc::Receiver<SubId>,
) {
    let url = mint_url
        .join_paths(&["v1", "ws"])
        .as_mut()
        .map(|url| {
            if url.scheme() == "https" {
                url.set_scheme("wss").expect("Could not set scheme");
            } else {
                url.set_scheme("ws").expect("Could not set scheme");
            }
            url
        })
        .expect("Could not join paths")
        .to_string();

    let mut active_subscriptions = HashMap::<SubId, mpsc::Sender<_>>::new();
    let mut failure_count = 0;

    loop {
        tracing::debug!("Connecting to {}", url);
        let ws_stream = match connect_async(&url).await {
            Ok((ws_stream, _)) => ws_stream,
            Err(err) => {
                failure_count += 1;
                tracing::error!("Could not connect to server: {:?}", err);
                if failure_count > MAX_ATTEMPT_FALLBACK_HTTP {
                    tracing::error!(
                        "Could not connect to server after {MAX_ATTEMPT_FALLBACK_HTTP} attempts, falling back to HTTP-subscription client"
                    );
                    return fallback_to_http(
                        active_subscriptions.into_keys(),
                        http_client,
                        subscriptions,
                        new_subscription_recv,
                        on_drop,
                    )
                    .await;
                }
                continue;
            }
        };
        tracing::debug!("Connected to {}", url);

        failure_count = 0;

        let (mut write, mut read) = ws_stream.split();
        let req_id = AtomicUsize::new(0);

        let get_sub_request = |params: Params| -> Option<(usize, String)> {
            let request: WsRequest = (
                WsMethodRequest::Subscribe(params),
                req_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            )
                .into();

            match serde_json::to_string(&request) {
                Ok(json) => Some((request.id, json)),
                Err(err) => {
                    tracing::error!("Could not serialize subscribe message: {:?}", err);
                    None
                }
            }
        };

        let get_unsub_request = |sub_id: SubId| -> Option<String> {
            let request: WsRequest = (
                WsMethodRequest::Unsubscribe(WsUnsubscribeRequest { sub_id }),
                req_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            )
                .into();

            match serde_json::to_string(&request) {
                Ok(json) => Some(json),
                Err(err) => {
                    tracing::error!("Could not serialize unsubscribe message: {:?}", err);
                    None
                }
            }
        };

        // Websocket reconnected, restore all subscriptions
        let mut subscription_requests = HashSet::new();

        let read_subscriptions = subscriptions.read().await;
        for (sub_id, _) in active_subscriptions.iter() {
            if let Some(Some((req_id, req))) = read_subscriptions
                .get(sub_id)
                .map(|(_, params)| get_sub_request(params.clone()))
            {
                let _ = write.send(Message::Text(req)).await;
                subscription_requests.insert(req_id);
            }
        }
        drop(read_subscriptions);

        loop {
            tokio::select! {
                Some(msg) = read.next() => {
                    let msg = match msg {
                        Ok(msg) => msg,
                        Err(_) => break,
                    };
                    let msg = match msg {
                        Message::Text(msg) => msg,
                        _ => continue,
                    };
                    let msg = match serde_json::from_str::<WsMessageOrResponse>(&msg) {
                        Ok(msg) => msg,
                        Err(_) => continue,
                    };

                    match msg {
                        WsMessageOrResponse::Notification(payload) => {
                            tracing::debug!("Received notification from server: {:?}", payload);
                            let _ = active_subscriptions.get(&payload.params.sub_id).map(|sender| {
                                let _ = sender.try_send(payload.params.payload);
                            });
                        }
                        WsMessageOrResponse::Response(response) => {
                            tracing::debug!("Received response from server: {:?}", response);
                            subscription_requests.remove(&response.id);
                        }
                        WsMessageOrResponse::ErrorResponse(error) => {
                            tracing::error!("Received error from server: {:?}", error);
                            if subscription_requests.contains(&error.id) {
                                // If the server sends an error response to a subscription request, we should
                                // fallback to HTTP.
                                // TODO: Add some retry before giving up to HTTP.
                                return fallback_to_http(
                                    active_subscriptions.into_keys(),
                                    http_client,
                                    subscriptions,
                                    new_subscription_recv,
                                    on_drop,
                                ).await;
                            }
                        }
                    }

                }
                Some(subid) = new_subscription_recv.recv() => {
                    let subscription = subscriptions.read().await;
                    let sub = if let Some(subscription) = subscription.get(&subid) {
                        subscription
                    } else {
                        continue
                    };
                    tracing::debug!("Subscribing to {:?}", sub.1);
                    active_subscriptions.insert(subid, sub.0.clone());
                    if let Some((req_id, json)) = get_sub_request(sub.1.clone()) {
                        let _ = write.send(Message::Text(json)).await;
                        subscription_requests.insert(req_id);
                    }
                },
                Some(subid) = on_drop.recv() => {
                    let mut subscription = subscriptions.write().await;
                    if let Some(sub) = subscription.remove(&subid) {
                        drop(sub);
                    }
                    tracing::debug!("Unsubscribing from {:?}", subid);
                    if let Some(json) = get_unsub_request(subid) {
                        let _ = write.send(Message::Text(json)).await;
                    }
                }
            }
        }
    }
}

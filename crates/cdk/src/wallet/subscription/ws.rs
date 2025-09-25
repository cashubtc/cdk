use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

use cdk_common::subscription::Params;
use cdk_common::ws::{WsMessageOrResponse, WsMethodRequest, WsRequest, WsUnsubscribeRequest};
#[cfg(feature = "auth")]
use cdk_common::{Method, RoutePath};
use futures::{SinkExt, StreamExt};
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use super::http::http_main;
use super::WsSubscriptionBody;
use crate::mint_url::MintUrl;
use crate::pub_sub::SubId;
use crate::wallet::MintConnector;
use crate::Wallet;

const MAX_ATTEMPT_FALLBACK_HTTP: usize = 10;

#[inline]
pub async fn ws_main(
    http_client: Arc<dyn MintConnector + Send + Sync>,
    mint_url: MintUrl,
    subscriptions: Arc<RwLock<HashMap<SubId, WsSubscriptionBody>>>,
    mut new_subscription_recv: mpsc::Receiver<SubId>,
    mut on_drop: mpsc::Receiver<SubId>,
    wallet: Arc<Wallet>,
) {
    let mut url = mint_url
        .join_paths(&["v1", "ws"])
        .expect("Could not join paths");

    if url.scheme() == "https" {
        url.set_scheme("wss").expect("Could not set scheme");
    } else {
        url.set_scheme("ws").expect("Could not set scheme");
    }

    let request = match url.to_string().into_client_request() {
        Ok(req) => req,
        Err(err) => {
            tracing::error!("Failed to create client request: {:?}", err);
            // Fallback to HTTP client if we can't create the WebSocket request
            return http_main(
                std::iter::empty(),
                http_client,
                subscriptions,
                new_subscription_recv,
                on_drop,
                wallet,
            )
            .await;
        }
    };

    let mut active_subscriptions = HashMap::<SubId, mpsc::Sender<_>>::new();
    let mut failure_count = 0;

    loop {
        let mut request_clone = request.clone();
        #[cfg(feature = "auth")]
        {
            let auth_wallet = http_client.get_auth_wallet().await;
            let token = match auth_wallet.as_ref() {
                Some(auth_wallet) => {
                    let endpoint = cdk_common::ProtectedEndpoint::new(Method::Get, RoutePath::Ws);
                    match auth_wallet.get_auth_for_request(&endpoint).await {
                        Ok(token) => token,
                        Err(err) => {
                            tracing::warn!("Failed to get auth token: {:?}", err);
                            None
                        }
                    }
                }
                None => None,
            };

            if let Some(auth_token) = token {
                let header_key = match &auth_token {
                    cdk_common::AuthToken::ClearAuth(_) => "Clear-auth",
                    cdk_common::AuthToken::BlindAuth(_) => "Blind-auth",
                };

                match auth_token.to_string().parse() {
                    Ok(header_value) => {
                        request_clone.headers_mut().insert(header_key, header_value);
                    }
                    Err(err) => {
                        tracing::warn!("Failed to parse auth token as header value: {:?}", err);
                    }
                }
            }
        }

        tracing::debug!("Connecting to {}", url);
        let ws_stream = match connect_async(request_clone.clone()).await {
            Ok((ws_stream, _)) => ws_stream,
            Err(err) => {
                failure_count += 1;
                tracing::error!("Could not connect to server: {:?}", err);
                if failure_count > MAX_ATTEMPT_FALLBACK_HTTP {
                    tracing::error!(
                        "Could not connect to server after {MAX_ATTEMPT_FALLBACK_HTTP} attempts, falling back to HTTP-subscription client"
                    );

                    return http_main(
                        active_subscriptions.into_keys(),
                        http_client,
                        subscriptions,
                        new_subscription_recv,
                        on_drop,
                        wallet,
                    )
                    .await;
                }
                continue;
            }
        };
        tracing::debug!("Connected to {}", url);

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
                let _ = write.send(Message::Text(req.into())).await;
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
                            // reset connection failure after a successful response from the serer
                            failure_count = 0;
                        }
                        WsMessageOrResponse::ErrorResponse(error) => {
                            tracing::error!("Received error from server: {:?}", error);

                            if subscription_requests.contains(&error.id) {
                                failure_count += 1;
                                if failure_count > MAX_ATTEMPT_FALLBACK_HTTP {
                                    tracing::error!(
                                        "Falling back to HTTP client"
                                    );

                                    return http_main(
                                        active_subscriptions.into_keys(),
                                        http_client,
                                        subscriptions,
                                        new_subscription_recv,
                                        on_drop,
                                        wallet,
                                    )
                                    .await;
                                }

                                break; // break connection to force a reconnection, to attempt to recover form this error
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
                        let _ = write.send(Message::Text(json.into())).await;
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
                        let _ = write.send(Message::Text(json.into())).await;
                    }
                }
            }
        }
    }
}

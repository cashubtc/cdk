//! Client for subscriptions
//!
//! Mint servers can send notifications to clients about changes in the state,
//! according to NUT-17, using the WebSocket protocol. This module provides a
//! subscription manager that allows clients to subscribe to notifications from
//! multiple mint servers using WebSocket or with a poll-based system, using
//! the HTTP client.
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, RwLock};

use cdk_common::nut17::ws::{
    WsMessageOrResponse, WsMethodRequest, WsRequest, WsUnsubscribeRequest,
};
use cdk_common::nut17::{Kind, NotificationId};
use cdk_common::pub_sub::remote_consumer::{
    Consumer, MessageToTransportLoop, RemoteActiveConsumer, SubscribeMessage, Transport,
};
use cdk_common::pub_sub::{Error as PubsubError, Event, Pubsub, Topic};
use cdk_common::subscription::WalletParams;
use cdk_common::CheckStateRequest;
#[cfg(feature = "auth")]
use cdk_common::{Method, RoutePath};
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_futures;

use crate::event::MintEvent;
use crate::mint_url::MintUrl;
use crate::wallet::MintConnector;

type NotificationPayload = crate::nuts::NotificationPayload<String>;

/// Type alias
pub type ActiveSubscription = RemoteActiveConsumer<SubscriptionClient>;

/// Subscription manager
///
/// This structure should be instantiated once per wallet at most. It is
/// cloneable since all its members are Arcs.
///
/// The main goal is to provide a single interface to manage multiple
/// subscriptions to many servers to subscribe to events. If supported, the
/// WebSocket method is used to subscribe to server-side events. Otherwise, a
/// poll-based system is used, where a background task fetches information about
/// the resource every few seconds and notifies subscribers of any change
/// upstream.
///
/// The subscribers have a simple-to-use interface, receiving an
/// ActiveSubscription struct, which can be used to receive updates and to
/// unsubscribe from updates automatically on the drop.
#[derive(Clone)]
pub struct SubscriptionManager {
    all_connections: Arc<RwLock<HashMap<MintUrl, Arc<Consumer<SubscriptionClient>>>>>,
    http_client: Arc<dyn MintConnector + Send + Sync>,
    prefer_http: bool,
}

impl Debug for SubscriptionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Subscription Manager connected to {:?}",
            self.all_connections
                .write()
                .map(|connections| connections.keys().cloned().collect::<Vec<_>>())
                .unwrap_or_default()
        )
    }
}

impl SubscriptionManager {
    /// Create a new subscription manager
    pub fn new(http_client: Arc<dyn MintConnector + Send + Sync>, prefer_http: bool) -> Self {
        Self {
            all_connections: Arc::new(RwLock::new(HashMap::new())),
            http_client,
            prefer_http,
        }
    }

    /// Subscribe to updates from a mint server with a given filter
    pub fn subscribe(
        &self,
        mint_url: MintUrl,
        filter: WalletParams,
    ) -> Result<RemoteActiveConsumer<SubscriptionClient>, PubsubError> {
        self.all_connections
            .write()
            .map_err(|_| PubsubError::Poison)?
            .entry(mint_url.clone())
            .or_insert_with(|| {
                Consumer::new(
                    SubscriptionClient {
                        mint_url,
                        http_client: self.http_client.clone(),
                        req_id: 0.into(),
                    },
                    self.prefer_http,
                    Pubsub::new(MintSubTopics {}),
                )
            })
            .subscribe(filter)
    }
}

/// MintSubTopics
#[derive(Clone)]
pub struct MintSubTopics {}

#[async_trait::async_trait]
impl Topic for MintSubTopics {
    type SubscriptionName = String;

    type Event = MintEvent<String>;

    async fn fetch_events(
        &self,
        _indexes: Vec<<Self::Event as Event>::Topic>,
        _sub_name: Self::SubscriptionName,
        _reply_to: mpsc::Sender<(Self::SubscriptionName, Self::Event)>,
    ) {
    }

    /// Store events or replace them
    async fn store_events(&self, _event: Self::Event) {}
}

/// Subscription client
///
/// If the server supports WebSocket subscriptions, this client will be used,
/// otherwise the HTTP pool and pause will be used (which is the less efficient
/// method).
#[derive(Debug)]
pub struct SubscriptionClient {
    http_client: Arc<dyn MintConnector + Send + Sync>,
    mint_url: MintUrl,
    req_id: AtomicUsize,
}

impl SubscriptionClient {
    fn get_sub_request(
        &self,
        id: String,
        params: NotificationId<String>,
    ) -> Option<(usize, String)> {
        let (kind, filter) = match params {
            NotificationId::ProofState(x) => (Kind::ProofState, x.to_string()),
            NotificationId::MeltQuoteBolt11(q) | NotificationId::MeltQuoteBolt12(q) => {
                (Kind::Bolt11MeltQuote, q)
            }
            NotificationId::MintQuoteBolt11(q) => (Kind::Bolt11MintQuote, q),
            NotificationId::MintQuoteBolt12(q) => (Kind::Bolt12MintQuote, q),
        };

        let request: WsRequest<_> = (
            WsMethodRequest::Subscribe(WalletParams {
                kind,
                filters: vec![filter],
                id,
            }),
            self.req_id
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        )
            .into();

        serde_json::to_string(&request)
            .inspect_err(|err| {
                tracing::error!("Could not serialize subscribe message: {:?}", err);
            })
            .map(|json| (request.id, json))
            .ok()
    }

    fn get_unsub_request(&self, sub_id: String) -> Option<String> {
        let request: WsRequest<_> = (
            WsMethodRequest::Unsubscribe(WsUnsubscribeRequest { sub_id }),
            self.req_id
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        )
            .into();

        match serde_json::to_string(&request) {
            Ok(json) => Some(json),
            Err(err) => {
                tracing::error!("Could not serialize unsubscribe message: {:?}", err);
                None
            }
        }
    }
}

#[async_trait::async_trait]
impl Transport for SubscriptionClient {
    type Topic = MintSubTopics;

    fn new_name(&self) -> <Self::Topic as Topic>::SubscriptionName {
        Uuid::new_v4().to_string()
    }

    async fn long_connection(
        &self,
        mut subscribe_changes: mpsc::Receiver<MessageToTransportLoop<Self::Topic>>,
        topics: Vec<SubscribeMessage<Self::Topic>>,
        reply_to: Arc<Pubsub<Self::Topic>>,
    ) -> Result<(), PubsubError>
    where
        Self: Sized,
    {
        let mut url = self
            .mint_url
            .join_paths(&["v1", "ws"])
            .expect("Could not join paths");

        if url.scheme() == "https" {
            url.set_scheme("wss").expect("Could not set scheme");
        } else {
            url.set_scheme("ws").expect("Could not set scheme");
        }

        #[cfg(not(feature = "auth"))]
        let request = url.to_string().into_client_request().map_err(|err| {
            tracing::error!("Failed to create client request: {:?}", err);
            // Fallback to HTTP client if we can't create the WebSocket request
            cdk_common::pub_sub::Error::NotSupported
        })?;

        #[cfg(feature = "auth")]
        let mut request = url.to_string().into_client_request().map_err(|err| {
            tracing::error!("Failed to create client request: {:?}", err);
            // Fallback to HTTP client if we can't create the WebSocket request
            cdk_common::pub_sub::Error::NotSupported
        })?;

        #[cfg(feature = "auth")]
        {
            let auth_wallet = self.http_client.get_auth_wallet().await;
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
                        request.headers_mut().insert(header_key, header_value);
                    }
                    Err(err) => {
                        tracing::warn!("Failed to parse auth token as header value: {:?}", err);
                    }
                }
            }
        }

        tracing::debug!("Connecting to {}", url);
        let ws_stream = connect_async(request)
            .await
            .map(|(ws_stream, _)| ws_stream)
            .map_err(|err| {
                tracing::error!("Error connecting: {err:?}");

                cdk_common::pub_sub::Error::Internal(Box::new(err))
            })?;

        tracing::debug!("Connected to {}", url);
        let (mut write, mut read) = ws_stream.split();

        for (name, index) in topics {
            let (_, req) = if let Some(req) = self.get_sub_request(name, index) {
                req
            } else {
                continue;
            };

            let _ = write.send(Message::Text(req.into())).await;
        }

        loop {
            tokio::select! {
                Some(msg) = subscribe_changes.recv() => {
                    match msg {
                        MessageToTransportLoop::Subscribe(msg) => {
                            let (_, req) = if let Some(req) = self.get_sub_request(msg.0, msg.1) {
                                req
                            } else {
                                continue;
                            };
                            let _ = write.send(Message::Text(req.into())).await;
                        }
                        MessageToTransportLoop::Unsubscribe(msg) => {
                            let req = if let Some(req) = self.get_unsub_request(msg) {
                                req
                            } else {
                                continue;
                            };
                            let _ = write.send(Message::Text(req.into())).await;
                        }
                        MessageToTransportLoop::Stop => {
                            return Ok(());
                        }
                    };
                }
                Some(msg) = read.next() => {
                    let msg = match msg {
                        Ok(msg) => msg,
                        Err(_) => break,
                    };
                    let msg = match msg {
                        Message::Text(msg) => msg,
                        _ => continue,
                    };
                    let msg = match serde_json::from_str::<WsMessageOrResponse<String>>(&msg) {
                        Ok(msg) => msg,
                        Err(_) => continue,
                    };

                    match msg {
                        WsMessageOrResponse::Notification(payload) => {
                            reply_to.publish(payload.params.payload);
                        }
                        WsMessageOrResponse::Response(response) => {
                            tracing::debug!("Received response from server: {:?}", response);
                        }
                        WsMessageOrResponse::ErrorResponse(error) => {
                            tracing::debug!("Received an error from server: {:?}", error);
                            return Err(PubsubError::InternalStr(error.error.message));
                        }
                    }

                }
            }
        }

        Ok(())
    }

    /// Poll on demand
    async fn poll(
        &self,
        topics: Vec<SubscribeMessage<Self::Topic>>,
        reply_to: Arc<Pubsub<Self::Topic>>,
    ) -> Result<(), PubsubError> {
        let proofs = topics
            .iter()
            .filter_map(|(_, x)| match &x {
                NotificationId::ProofState(p) => Some(*p),
                _ => None,
            })
            .collect::<Vec<_>>();

        if !proofs.is_empty() {
            for state in self
                .http_client
                .post_check_state(CheckStateRequest { ys: proofs })
                .await
                .map_err(|e| PubsubError::Internal(Box::new(e)))?
                .states
            {
                reply_to.publish(MintEvent::new(NotificationPayload::ProofState(state)));
            }
        }

        for topic in topics
            .into_iter()
            .map(|(_, x)| x)
            .filter(|x| !matches!(x, NotificationId::ProofState(_)))
        {
            match topic {
                NotificationId::MintQuoteBolt11(id) => {
                    let response = match self.http_client.get_mint_quote_status(&id).await {
                        Ok(success) => success,
                        Err(err) => {
                            tracing::error!("Error with MintBolt11 {} with {:?}", id, err);
                            continue;
                        }
                    };

                    reply_to.publish(MintEvent::new(
                        NotificationPayload::MintQuoteBolt11Response(response.clone()),
                    ));
                }
                NotificationId::MeltQuoteBolt11(id) => {
                    let response = match self.http_client.get_melt_quote_status(&id).await {
                        Ok(success) => success,
                        Err(err) => {
                            tracing::error!("Error with MeltBolt11 {} with {:?}", id, err);
                            continue;
                        }
                    };

                    let _ = reply_to.publish(MintEvent::new(
                        NotificationPayload::MeltQuoteBolt11Response(response),
                    ));
                }
                NotificationId::MintQuoteBolt12(id) => {
                    let response = match self.http_client.get_mint_quote_bolt12_status(&id).await {
                        Ok(success) => success,
                        Err(err) => {
                            tracing::error!("Error with MintBolt12 {} with {:?}", id, err);
                            continue;
                        }
                    };

                    reply_to.publish(MintEvent::new(
                        NotificationPayload::MintQuoteBolt12Response(response),
                    ));
                }
                NotificationId::MeltQuoteBolt12(id) => {
                    let response = match self.http_client.get_melt_bolt12_quote_status(&id).await {
                        Ok(success) => success,
                        Err(err) => {
                            tracing::error!("Error with MeltBolt12 {} with {:?}", id, err);
                            continue;
                        }
                    };

                    reply_to.publish(MintEvent::new(
                        NotificationPayload::MeltQuoteBolt11Response(response),
                    ));
                }
                _ => {}
            }
        }

        Ok(())
    }
}

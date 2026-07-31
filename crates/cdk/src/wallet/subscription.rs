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
use std::sync::Arc;

use cdk_common::nut00::KnownMethod;
use cdk_common::nut17::ws::{
    RawWsMessageOrResponse, WsAuthenticateRequest, WsMethodRequest, WsRequest, WsResponseResult,
    WsUnsubscribeRequest,
};
use cdk_common::nut17::{deserialize_payload_for_kind, Kind, NotificationId};
use cdk_common::parking_lot::RwLock;
use cdk_common::pub_sub::remote_consumer::{
    Consumer, InternalRelay, RemoteActiveConsumer, StreamCtrl, SubscribeMessage, Transport,
};
use cdk_common::pub_sub::{Error as PubsubError, Spec, Subscriber};
use cdk_common::subscription::WalletParams;
use cdk_common::ws_client::WsError;
use cdk_common::{AuthRequired, CheckStateRequest, Method, PaymentMethod, RoutePath};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::event::MintEvent;
use crate::mint_url::MintUrl;
use crate::wallet::auth::AuthWallet;
use crate::wallet::MintConnector;

/// Notification Payload
pub type NotificationPayload = crate::nuts::NotificationPayload<String>;

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
                .keys()
                .cloned()
                .collect::<Vec<_>>()
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
            .entry(mint_url.clone())
            .or_insert_with(|| {
                Consumer::new(
                    SubscriptionClient {
                        mint_url,
                        http_client: self.http_client.clone(),
                        req_id: 0.into(),
                    },
                    self.prefer_http,
                    (),
                )
            })
            .subscribe(filter)
    }
}

/// MintSubTopics
#[derive(Clone, Default, Debug)]
pub struct MintSubTopics {}

#[async_trait::async_trait]
impl Spec for MintSubTopics {
    type SubscriptionId = String;

    type Event = MintEvent<String>;

    type Topic = NotificationId<String>;

    type Context = ();

    fn new_instance(_context: Self::Context) -> Arc<Self>
    where
        Self: Sized,
    {
        Arc::new(Self {})
    }

    async fn fetch_events(self: &Arc<Self>, _topics: Vec<Self::Topic>, _reply_to: Subscriber<Self>)
    where
        Self: Sized,
    {
    }
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
    fn subscription_kind(params: &NotificationId<String>) -> Kind {
        match params {
            NotificationId::ProofState(_) => Kind::ProofState,
            NotificationId::MeltQuoteBolt11(_) => Kind::Bolt11MeltQuote,
            NotificationId::MeltQuoteBolt12(_) => Kind::Bolt12MeltQuote,
            NotificationId::MintQuoteBolt11(_) => Kind::Bolt11MintQuote,
            NotificationId::MintQuoteBolt12(_) => Kind::Bolt12MintQuote,
            NotificationId::MintQuoteOnchain(_) => Kind::OnchainMintQuote,
            NotificationId::MeltQuoteOnchain(_) => Kind::OnchainMeltQuote,
            NotificationId::MintQuoteCustom(method, _) => {
                Kind::Custom(format!("{}_mint_quote", method))
            }
            NotificationId::MeltQuoteCustom(method, _) => {
                Kind::Custom(format!("{}_melt_quote", method))
            }
        }
    }

    fn get_sub_request(
        &self,
        id: String,
        params: NotificationId<String>,
    ) -> Option<(usize, String)> {
        let kind = Self::subscription_kind(&params);
        let filter = match params {
            NotificationId::ProofState(x) => x.to_string(),
            NotificationId::MeltQuoteBolt11(q)
            | NotificationId::MeltQuoteBolt12(q)
            | NotificationId::MintQuoteBolt11(q)
            | NotificationId::MintQuoteBolt12(q)
            | NotificationId::MintQuoteOnchain(q)
            | NotificationId::MeltQuoteOnchain(q)
            | NotificationId::MintQuoteCustom(_, q)
            | NotificationId::MeltQuoteCustom(_, q) => q,
        };

        let request: WsRequest<_> = (
            WsMethodRequest::Subscribe(WalletParams {
                kind,
                filters: vec![filter],
                id: id.into(),
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

    fn get_unsub_request(&self, sub_id: String) -> Option<(usize, String)> {
        let request: WsRequest<_> = (
            WsMethodRequest::Unsubscribe(WsUnsubscribeRequest { sub_id }),
            self.req_id
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        )
            .into();

        serde_json::to_string(&request)
            .inspect_err(|err| {
                tracing::error!("Could not serialize unsubscribe message: {:?}", err);
            })
            .map(|json| (request.id, json))
            .ok()
    }

    /// Build a NUT-22 `authenticate` command carrying the serialized BAT.
    fn get_auth_request(&self, token: String) -> Option<(usize, String)> {
        let request: WsRequest<String> = (
            WsMethodRequest::Authenticate(WsAuthenticateRequest { token }),
            self.req_id
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        )
            .into();

        serde_json::to_string(&request)
            .inspect_err(|err| {
                tracing::error!("Could not serialize authenticate message: {:?}", err);
            })
            .map(|json| (request.id, json))
            .ok()
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl Transport for SubscriptionClient {
    type Spec = MintSubTopics;

    fn new_name(&self) -> <Self::Spec as Spec>::SubscriptionId {
        new_subscription_id()
    }

    async fn stream(
        &self,
        ctrls: mpsc::Receiver<StreamCtrl<Self::Spec>>,
        topics: Vec<SubscribeMessage<Self::Spec>>,
        reply_to: InternalRelay<Self::Spec>,
    ) -> Result<(), PubsubError> {
        stream_client(self, ctrls, topics, reply_to).await
    }

    /// Poll on demand
    async fn poll(
        &self,
        topics: Vec<SubscribeMessage<Self::Spec>>,
        reply_to: InternalRelay<Self::Spec>,
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
                reply_to.send(MintEvent::new(NotificationPayload::ProofState(state)));
            }
        }

        for topic in topics
            .into_iter()
            .map(|(_, x)| x)
            .filter(|x| !matches!(x, NotificationId::ProofState(_)))
        {
            match topic {
                NotificationId::MintQuoteBolt11(id) => {
                    let response = match self
                        .http_client
                        .get_mint_quote_status(PaymentMethod::BOLT11, &id)
                        .await
                    {
                        Ok(success) => match success {
                            cdk_common::MintQuoteResponse::Bolt11(r) => r,
                            _ => {
                                tracing::error!("Unexpected response type for MintBolt11 {}", id);
                                continue;
                            }
                        },
                        Err(err) => {
                            tracing::error!("Error with MintBolt11 {} with {:?}", id, err);
                            continue;
                        }
                    };

                    reply_to.send(MintEvent::new(
                        NotificationPayload::MintQuoteBolt11Response(response),
                    ));
                }
                NotificationId::MeltQuoteBolt11(id) => {
                    let response = match self
                        .http_client
                        .get_melt_quote_status(PaymentMethod::BOLT11, &id)
                        .await
                    {
                        Ok(success) => match success {
                            cdk_common::MeltQuoteResponse::Bolt11(r) => r,
                            _ => {
                                tracing::error!("Unexpected response type for MeltBolt11 {}", id);
                                continue;
                            }
                        },
                        Err(err) => {
                            tracing::error!("Error with MeltBolt11 {} with {:?}", id, err);
                            continue;
                        }
                    };

                    reply_to.send(MintEvent::new(
                        NotificationPayload::MeltQuoteBolt11Response(response),
                    ));
                }
                NotificationId::MintQuoteBolt12(id) => {
                    let response = match self
                        .http_client
                        .get_mint_quote_status(PaymentMethod::BOLT12, &id)
                        .await
                    {
                        Ok(success) => match success {
                            cdk_common::MintQuoteResponse::Bolt12(r) => r,
                            _ => {
                                tracing::error!("Unexpected response type for MintBolt12 {}", id);
                                continue;
                            }
                        },
                        Err(err) => {
                            tracing::error!("Error with MintBolt12 {} with {:?}", id, err);
                            continue;
                        }
                    };

                    reply_to.send(MintEvent::new(
                        NotificationPayload::MintQuoteBolt12Response(response),
                    ));
                }
                NotificationId::MeltQuoteBolt12(id) => {
                    let response = match self
                        .http_client
                        .get_melt_quote_status(PaymentMethod::BOLT12, &id)
                        .await
                    {
                        Ok(success) => match success {
                            cdk_common::MeltQuoteResponse::Bolt12(r) => r,
                            _ => {
                                tracing::error!("Unexpected response type for MeltBolt12 {}", id);
                                continue;
                            }
                        },
                        Err(err) => {
                            tracing::error!("Error with MeltBolt12 {} with {:?}", id, err);
                            continue;
                        }
                    };

                    reply_to.send(MintEvent::new(
                        NotificationPayload::MeltQuoteBolt12Response(response),
                    ));
                }
                NotificationId::MintQuoteOnchain(id) => {
                    let response = match self
                        .http_client
                        .get_mint_quote_status(PaymentMethod::Known(KnownMethod::Onchain), &id)
                        .await
                    {
                        Ok(success) => match success {
                            cdk_common::MintQuoteResponse::Onchain(r) => r,
                            _ => {
                                tracing::error!("Unexpected response type for MintOnchain {}", id);
                                continue;
                            }
                        },
                        Err(err) => {
                            tracing::error!("Error with MintOnchain {} with {:?}", id, err);
                            continue;
                        }
                    };

                    reply_to.send(MintEvent::new(
                        NotificationPayload::MintQuoteOnchainResponse(response),
                    ));
                }
                NotificationId::MeltQuoteOnchain(id) => {
                    let response = match self
                        .http_client
                        .get_melt_quote_status(PaymentMethod::Known(KnownMethod::Onchain), &id)
                        .await
                    {
                        Ok(success) => match success {
                            cdk_common::MeltQuoteResponse::Onchain(r) => r,
                            _ => {
                                tracing::error!("Unexpected response type for MeltOnchain {}", id);
                                continue;
                            }
                        },
                        Err(err) => {
                            tracing::error!("Error with MeltOnchain {} with {:?}", id, err);
                            continue;
                        }
                    };

                    reply_to.send(MintEvent::new(
                        NotificationPayload::MeltQuoteOnchainResponse(response),
                    ));
                }
                NotificationId::MintQuoteCustom(method, id) => {
                    let response = match self
                        .http_client
                        .get_mint_quote_status(PaymentMethod::Custom(method.clone()), &id)
                        .await
                    {
                        Ok(success) => match success {
                            cdk_common::MintQuoteResponse::Custom { response, .. } => response,
                            _ => {
                                tracing::error!(
                                    "Unexpected response type for Custom Mint Quote {}",
                                    id
                                );
                                continue;
                            }
                        },
                        Err(err) => {
                            tracing::error!("Error with Custom Mint Quote {} with {:?}", id, err);
                            continue;
                        }
                    };

                    reply_to.send(MintEvent::new(
                        NotificationPayload::CustomMintQuoteResponse(method, response),
                    ));
                }
                NotificationId::MeltQuoteCustom(method, id) => {
                    let response = match self
                        .http_client
                        .get_melt_quote_status(PaymentMethod::Custom(method.clone()), &id)
                        .await
                    {
                        Ok(success) => match success {
                            cdk_common::MeltQuoteResponse::Custom((_, r)) => r,
                            _ => {
                                tracing::error!(
                                    "Unexpected response type for Custom Melt Quote {}",
                                    id
                                );
                                continue;
                            }
                        },
                        Err(err) => {
                            tracing::error!("Error with Custom Melt Quote {} with {:?}", id, err);
                            continue;
                        }
                    };

                    reply_to.send(MintEvent::new(
                        NotificationPayload::CustomMeltQuoteResponse(method, response),
                    ));
                }
                _ => {}
            }
        }

        Ok(())
    }
}

fn new_subscription_id() -> String {
    Uuid::now_v7().to_string()
}

/// Authenticate the connection with a blind auth token, once, just before the
/// first subscribe.
///
/// A single BAT authenticates the connection for its lifetime, so this fetches
/// and spends a token only on the first call and only when the endpoint needs
/// blind auth. Because it runs after a successful connect and only when a
/// subscribe is about to be sent, a failed connect never burns a BAT.
async fn ensure_authenticated(
    client: &SubscriptionClient,
    sender: &mut cdk_common::ws_client::WsSender,
    pending_requests: &mut HashMap<usize, PendingRequest>,
    auth_wallet: Option<&AuthWallet>,
    endpoint: &cdk_common::ProtectedEndpoint,
    needs_blind_auth: bool,
    authenticated: &mut bool,
) -> Result<(), PubsubError> {
    if !needs_blind_auth || *authenticated {
        return Ok(());
    }

    let wallet = auth_wallet.ok_or_else(|| {
        PubsubError::InternalStr("blind auth required but no auth wallet".to_string())
    })?;

    let token = wallet
        .get_auth_for_request(endpoint)
        .await
        .map_err(|err| {
            PubsubError::InternalStr(format!("failed to get blind auth token: {err:?}"))
        })?
        .ok_or_else(|| {
            PubsubError::InternalStr("blind auth required but no token available".to_string())
        })?;

    let (request_id, req) = client.get_auth_request(token.to_string()).ok_or_else(|| {
        PubsubError::InternalStr("failed to build authenticate request".to_string())
    })?;

    // Only mark the connection authenticated once the command is actually on the
    // wire. The BAT has already been spent from the wallet store, so a failed
    // send must surface as an error (reconnect) rather than silently proceeding
    // as if authenticated.
    sender.send(req).await.map_err(map_ws_error)?;
    pending_requests.insert(request_id, PendingRequest::Authenticate);
    *authenticated = true;
    Ok(())
}

/// Send a subscribe request and record its kind for notification decoding.
async fn send_subscribe(
    client: &SubscriptionClient,
    sender: &mut cdk_common::ws_client::WsSender,
    sub_id_to_kind: &mut HashMap<String, Kind>,
    pending_requests: &mut HashMap<usize, PendingRequest>,
    name: String,
    index: NotificationId<String>,
) -> Result<(), PubsubError> {
    let kind = SubscriptionClient::subscription_kind(&index);
    let Some((request_id, req)) = client.get_sub_request(name.clone(), index) else {
        return Ok(());
    };

    sub_id_to_kind.insert(name.clone(), kind);
    sender.send(req).await.map_err(map_ws_error)?;
    pending_requests.insert(request_id, PendingRequest::Subscribe { sub_id: name });
    Ok(())
}

async fn stream_client(
    client: &SubscriptionClient,
    mut ctrl: mpsc::Receiver<StreamCtrl<MintSubTopics>>,
    topics: Vec<SubscribeMessage<MintSubTopics>>,
    reply_to: InternalRelay<MintSubTopics>,
) -> Result<(), PubsubError> {
    let mut sub_id_to_kind = HashMap::new();
    let mut pending_requests = HashMap::new();

    let mut url = client
        .mint_url
        .join_paths(&["v1", "ws"])
        .expect("Could not join paths");

    if url.scheme() == "https" {
        url.set_scheme("wss").expect("Could not set scheme");
    } else {
        url.set_scheme("ws").expect("Could not set scheme");
    }

    let endpoint = cdk_common::ProtectedEndpoint::new(Method::Get, RoutePath::Ws);
    let auth_wallet = client.http_client.get_auth_wallet().await;

    // Learn the auth requirement without consuming a token. Only clear auth
    // travels in a header; blind auth is done in-band, and the BAT is fetched
    // lazily just before the first subscribe (see `ensure_authenticated`), so a
    // failed connect never burns a single-use BAT.
    let auth_required = match auth_wallet.as_ref() {
        Some(wallet) => wallet.is_protected(&endpoint).await,
        None => None,
    };

    let mut headers: Vec<(&str, String)> = Vec::new();
    if matches!(auth_required, Some(AuthRequired::Clear)) {
        if let Some(wallet) = auth_wallet.as_ref() {
            match wallet.get_auth_for_request(&endpoint).await {
                Ok(Some(token)) => headers.push(("Clear-auth", token.to_string())),
                Ok(None) => {}
                Err(err) => tracing::warn!("Failed to get clear auth token: {:?}", err),
            }
        }
    }

    let needs_blind_auth = matches!(auth_required, Some(AuthRequired::Blind));

    let url_str = url.to_string();
    let header_refs: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (*k, v.as_str())).collect();

    tracing::debug!("Connecting to {}", url);
    let (mut sender, mut receiver) = client
        .http_client
        .connect_websocket(&url_str, &header_refs)
        .await
        .map_err(|err| {
            tracing::error!("Error connecting: {err:?}");
            map_ws_error(err)
        })?;

    tracing::debug!("Connected to {}", url);

    // Whether `authenticate` has been sent on this connection. A single BAT
    // authenticates the connection for its lifetime, so we send it once, lazily,
    // just before the first subscribe (a connection with no subscriptions never
    // authenticates and the mint closes it after its auth timeout, which is
    // fine).
    let mut authenticated = false;

    for (name, index) in topics {
        ensure_authenticated(
            client,
            &mut sender,
            &mut pending_requests,
            auth_wallet.as_ref(),
            &endpoint,
            needs_blind_auth,
            &mut authenticated,
        )
        .await?;
        send_subscribe(
            client,
            &mut sender,
            &mut sub_id_to_kind,
            &mut pending_requests,
            name,
            index,
        )
        .await?;
    }

    loop {
        tokio::select! {
            Some(msg) = ctrl.recv() => {
                match msg {
                    StreamCtrl::Subscribe(msg) => {
                        ensure_authenticated(
                            client,
                            &mut sender,
                            &mut pending_requests,
                            auth_wallet.as_ref(),
                            &endpoint,
                            needs_blind_auth,
                            &mut authenticated,
                        )
                        .await?;
                        send_subscribe(
                            client,
                            &mut sender,
                            &mut sub_id_to_kind,
                            &mut pending_requests,
                            msg.0,
                            msg.1,
                        )
                        .await?;
                    }
                    StreamCtrl::Unsubscribe(msg) => {
                        sub_id_to_kind.remove(&msg);
                        let (request_id, req) = if let Some(req) = client.get_unsub_request(msg.clone()) {
                            req
                        } else {
                            continue;
                        };
                        sender.send(req).await.map_err(map_ws_error)?;
                        pending_requests.insert(
                            request_id,
                            PendingRequest::Unsubscribe { sub_id: msg },
                        );
                    }
                    StreamCtrl::Stop => {
                        if let Err(err) = sender.close().await {
                            tracing::error!("Closing error {err:?}");
                        }
                        return Ok(());
                    }
                };
            }
            msg = receiver.recv() => {
                let msg = match msg {
                    Some(Ok(msg)) => msg,
                    Some(Err(err)) => {
                        if let Err(err) = sender.close().await {
                            tracing::error!("Closing error {err:?}");
                        }
                        return Err(map_ws_error(err));
                    }
                    None => {
                        return Err(PubsubError::InternalStr(
                            "WebSocket stream closed unexpectedly".to_string(),
                        ));
                    }
                };
                let msg = match serde_json::from_str::<RawWsMessageOrResponse<String>>(&msg) {
                    Ok(msg) => msg,
                    Err(_) => continue,
                };

                match msg {
                    RawWsMessageOrResponse::Notification(ref payload) => {
                        let Some(kind) = sub_id_to_kind.get(&payload.params.sub_id) else {
                            tracing::warn!(
                                "Received websocket notification for unknown subId {}",
                                payload.params.sub_id
                            );
                            continue;
                        };

                        if let Some(payload) = decode_notification_payload_for_stream(
                            kind,
                            &payload.params.sub_id,
                            payload.params.payload.clone(),
                        ) {
                            reply_to.send(payload);
                        }
                    }
                    RawWsMessageOrResponse::Response(response) => {
                        let Some(request) = pending_requests.remove(&response.id) else {
                            tracing::warn!(
                                "Received websocket response for unknown request id {}",
                                response.id
                            );
                            continue;
                        };

                        match response.result {
                            WsResponseResult::Authenticate(_) => {
                                if !matches!(request, PendingRequest::Authenticate) {
                                    tracing::warn!(
                                        "Received authenticate response for a {} request",
                                        request.method()
                                    );
                                    continue;
                                }

                                tracing::debug!("Websocket connection authenticated");
                            }
                            WsResponseResult::Subscription(result) => {
                                if Some(result.sub_id.as_str()) != request.sub_id() {
                                    tracing::warn!(
                                        "Received {} response for subId {}, expected {:?}",
                                        request.method(),
                                        result.sub_id,
                                        request.sub_id()
                                    );
                                    continue;
                                }

                                tracing::debug!(
                                    "Received {} response from server for subId {} with status {}",
                                    request.method(),
                                    result.sub_id,
                                    result.status
                                );
                            }
                        }
                    }
                    RawWsMessageOrResponse::ErrorResponse(error) => {
                        match pending_requests.remove(&error.id) {
                            Some(request) => tracing::debug!(
                                "Received an error from server for {} request and subId {:?}: {}",
                                request.method(),
                                request.sub_id(),
                                error.error.message
                            ),
                            None => tracing::debug!(
                                "Received an error from server for unknown request id {}: {}",
                                error.id,
                                error.error.message
                            ),
                        }
                        return Err(PubsubError::InternalStr(error.error.message));
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PendingRequest {
    Subscribe { sub_id: String },
    Unsubscribe { sub_id: String },
    Authenticate,
}

impl PendingRequest {
    fn method(&self) -> &'static str {
        match self {
            Self::Subscribe { .. } => "subscribe",
            Self::Unsubscribe { .. } => "unsubscribe",
            Self::Authenticate => "authenticate",
        }
    }

    fn sub_id(&self) -> Option<&str> {
        match self {
            Self::Subscribe { sub_id } | Self::Unsubscribe { sub_id } => Some(sub_id),
            Self::Authenticate => None,
        }
    }
}

fn decode_notification_payload_for_stream(
    kind: &Kind,
    sub_id: &str,
    payload: serde_json::Value,
) -> Option<NotificationPayload> {
    match deserialize_payload_for_kind::<String, serde_json::Error>(kind, payload) {
        Ok(payload) => Some(payload),
        Err(err) => {
            tracing::warn!(
                "Dropping unsupported websocket notification for subId {}: {}",
                sub_id,
                err
            );
            None
        }
    }
}

fn map_ws_error(err: WsError) -> PubsubError {
    match err {
        WsError::Transient(message) => PubsubError::InternalStr(message),
        WsError::NotSupported(_) => PubsubError::NotSupported,
        WsError::Terminal(message) => PubsubError::Terminal(message),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn nut17_subscription_ids_use_uuid_v7() {
        let id = Uuid::parse_str(&new_subscription_id()).expect("valid subscription UUID");

        assert_eq!(id.get_version_num(), 7);
    }

    #[test]
    fn unsupported_notification_does_not_block_the_next_notification() {
        let unsupported = json!({
            "Y": "02194603ffa062682c4f10e2dfe8f53e17d5d0329db51c8d3935cc74a4c0e0d4cb",
            "state": "FUTURE_STATE",
            "witness": null
        });
        let supported = json!({
            "Y": "02194603ffa062682c4f10e2dfe8f53e17d5d0329db51c8d3935cc74a4c0e0d4cb",
            "state": "UNSPENT",
            "witness": null
        });

        assert!(
            decode_notification_payload_for_stream(&Kind::ProofState, "sub-id", unsupported)
                .is_none()
        );
        assert!(matches!(
            decode_notification_payload_for_stream(&Kind::ProofState, "sub-id", supported),
            Some(NotificationPayload::ProofState(_))
        ));
    }

    #[test]
    fn transient_websocket_failure_keeps_streaming_enabled() {
        let error = map_ws_error(WsError::Transient("temporary disconnect".to_string()));

        assert!(matches!(error, PubsubError::InternalStr(_)));
    }

    #[test]
    fn unsupported_websocket_failure_disables_streaming() {
        let error = map_ws_error(WsError::NotSupported("404".to_string()));

        assert!(matches!(error, PubsubError::NotSupported));
    }

    #[test]
    fn terminal_websocket_failure_disables_streaming() {
        let error = map_ws_error(WsError::Terminal("attestation failed".to_string()));

        assert!(matches!(error, PubsubError::Terminal(_)));
    }

    #[test]
    fn decode_proof_state_notification() {
        let payload = json!({
            "Y": "02194603ffa062682c4f10e2dfe8f53e17d5d0329db51c8d3935cc74a4c0e0d4cb",
            "state": "UNSPENT",
            "witness": null
        });

        let decoded =
            deserialize_payload_for_kind::<String, serde_json::Error>(&Kind::ProofState, payload)
                .unwrap();

        assert!(matches!(decoded, NotificationPayload::ProofState(_)));
    }

    #[test]
    fn decode_bolt11_notifications() {
        let mint_payload = json!({
            "quote": "mint-quote",
            "request": "lnbc1...",
            "method": "bolt11",
            "state": "PAID",
            "expiry": 1234,
            "paid": true
        });
        let melt_payload = json!({
            "quote": "melt-quote",
            "amount": 21,
            "fee_reserve": 1,
            "method": "bolt11",
            "state": "PAID",
            "expiry": 1234,
            "payment_proof": "abc"
        });

        let mint_decoded = deserialize_payload_for_kind::<String, serde_json::Error>(
            &Kind::Bolt11MintQuote,
            mint_payload,
        )
        .unwrap();
        let melt_decoded = deserialize_payload_for_kind::<String, serde_json::Error>(
            &Kind::Bolt11MeltQuote,
            melt_payload,
        )
        .unwrap();

        assert!(matches!(
            mint_decoded,
            NotificationPayload::MintQuoteBolt11Response(_)
        ));
        assert!(matches!(
            melt_decoded,
            NotificationPayload::MeltQuoteBolt11Response(_)
        ));
    }

    #[test]
    fn decode_bolt12_notification() {
        let mint_payload = json!({
            "quote": "quote-id",
            "request": "lni1...",
            "amount": null,
            "unit": "sat",
            "method": "bolt12",
            "expiry": 1234,
            "pubkey": "02194603ffa062682c4f10e2dfe8f53e17d5d0329db51c8d3935cc74a4c0e0d4cb",
            "amount_paid": 0,
            "amount_issued": 0
        });
        let melt_payload = json!({
            "quote": "melt-quote",
            "amount": 21,
            "fee_reserve": 1,
            "state": "PAID",
            "expiry": 1234,
            "request": "lni1...",
            "unit": "sat"
        });

        let mint_decoded = deserialize_payload_for_kind::<String, serde_json::Error>(
            &Kind::Bolt12MintQuote,
            mint_payload,
        )
        .unwrap();
        let melt_decoded = deserialize_payload_for_kind::<String, serde_json::Error>(
            &Kind::Bolt12MeltQuote,
            melt_payload,
        )
        .unwrap();

        assert!(matches!(
            mint_decoded,
            NotificationPayload::MintQuoteBolt12Response(_)
        ));
        match melt_decoded {
            NotificationPayload::MeltQuoteBolt12Response(response) => {
                assert_eq!(response.method, PaymentMethod::BOLT12);
            }
            _ => panic!("expected bolt12 melt response"),
        }
    }

    #[test]
    fn decode_onchain_mint_notification_uses_subscription_kind() {
        let payload = json!({
            "quote": "onchain-quote",
            "request": "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh",
            "unit": "sat",
            "expiry": 1234,
            "pubkey": "02194603ffa062682c4f10e2dfe8f53e17d5d0329db51c8d3935cc74a4c0e0d4cb",
            "amount_paid": 0,
            "amount_issued": 0,
            "state": "future-extension"
        });

        let decoded = deserialize_payload_for_kind::<String, serde_json::Error>(
            &Kind::OnchainMintQuote,
            payload,
        )
        .unwrap();

        assert!(matches!(
            decoded,
            NotificationPayload::MintQuoteOnchainResponse(_)
        ));
    }

    #[test]
    fn decode_custom_notifications() {
        let mint_method = "foo".to_string();
        let melt_method = "foo".to_string();
        let mint_kind = Kind::Custom(format!("{}_mint_quote", mint_method));
        let melt_kind = Kind::Custom(format!("{}_melt_quote", melt_method));
        let mint_payload = json!({
            "quote": "mint-custom",
            "request": "custom-request",
            "amount": 42,
            "unit": "sat",
            "amount_paid": 0,
            "amount_issued": 0,
            "expiry": 1234,
            "pubkey": null,
            "extra_field": "value"
        });
        let melt_payload = json!({
            "quote": "melt-custom",
            "amount": 42,
            "fee_reserve": 1,
            "state": "PAID",
            "expiry": 1234,
            "payment_proof": null,
            "request": "custom-request",
            "unit": "sat",
            "extra_field": "value"
        });

        let mint_decoded =
            deserialize_payload_for_kind::<String, serde_json::Error>(&mint_kind, mint_payload)
                .unwrap();
        let melt_decoded =
            deserialize_payload_for_kind::<String, serde_json::Error>(&melt_kind, melt_payload)
                .unwrap();

        assert!(matches!(
            mint_decoded,
            NotificationPayload::CustomMintQuoteResponse(method, response)
                if method == mint_method && response.method == "foo"
        ));
        assert!(matches!(
            melt_decoded,
            NotificationPayload::CustomMeltQuoteResponse(method, response)
                if method == melt_method && response.method == "foo"
        ));
    }

    #[test]
    fn decode_unknown_custom_kind_errors() {
        assert!(deserialize_payload_for_kind::<String, serde_json::Error>(
            &Kind::Custom("foo_status".to_string()),
            json!({}),
        )
        .is_err());
    }

    #[test]
    fn decode_wrong_kind_errors() {
        let payload = json!({
            "Y": "02194603ffa062682c4f10e2dfe8f53e17d5d0329db51c8d3935cc74a4c0e0d4cb",
            "state": "UNSPENT",
            "witness": null
        });

        assert!(deserialize_payload_for_kind::<String, serde_json::Error>(
            &Kind::Bolt12MintQuote,
            payload,
        )
        .is_err());
    }
}

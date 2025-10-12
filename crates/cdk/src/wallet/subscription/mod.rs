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

use cdk_common::nut17::ws::{WsMethodRequest, WsRequest, WsUnsubscribeRequest};
use cdk_common::nut17::{Kind, NotificationId};
use cdk_common::parking_lot::RwLock;
use cdk_common::pub_sub::remote_consumer::{
    Consumer, InternalRelay, RemoteActiveConsumer, StreamCtrl, SubscribeMessage, Transport,
};
use cdk_common::pub_sub::{Error as PubsubError, Spec, Subscriber};
use cdk_common::subscription::WalletParams;
use cdk_common::CheckStateRequest;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::event::MintEvent;
use crate::mint_url::MintUrl;
use crate::wallet::MintConnector;

#[cfg(not(target_arch = "wasm32"))]
mod ws;

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
#[derive(Clone, Default)]
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
#[allow(dead_code)]
pub struct SubscriptionClient {
    http_client: Arc<dyn MintConnector + Send + Sync>,
    mint_url: MintUrl,
    req_id: AtomicUsize,
}

#[allow(dead_code)]
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

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl Transport for SubscriptionClient {
    type Spec = MintSubTopics;

    fn new_name(&self) -> <Self::Spec as Spec>::SubscriptionId {
        Uuid::new_v4().to_string()
    }

    async fn stream(
        &self,
        _ctrls: mpsc::Receiver<StreamCtrl<Self::Spec>>,
        _topics: Vec<SubscribeMessage<Self::Spec>>,
        _reply_to: InternalRelay<Self::Spec>,
    ) -> Result<(), PubsubError> {
        #[cfg(not(target_arch = "wasm32"))]
        let r = ws::stream_client(self, _ctrls, _topics, _reply_to).await;

        #[cfg(target_arch = "wasm32")]
        let r = Err(PubsubError::NotSupported);

        r
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
                    let response = match self.http_client.get_mint_quote_status(&id).await {
                        Ok(success) => success,
                        Err(err) => {
                            tracing::error!("Error with MintBolt11 {} with {:?}", id, err);
                            continue;
                        }
                    };

                    reply_to.send(MintEvent::new(
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

                    reply_to.send(MintEvent::new(
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

                    reply_to.send(MintEvent::new(
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

                    reply_to.send(MintEvent::new(
                        NotificationPayload::MeltQuoteBolt11Response(response),
                    ));
                }
                _ => {}
            }
        }

        Ok(())
    }
}

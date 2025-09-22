//! Client for subscriptions
//!
//! Mint servers can send notifications to clients about changes in the state,
//! according to NUT-17, using the WebSocket protocol. This module provides a
//! subscription manager that allows clients to subscribe to notifications from
//! multiple mint servers using WebSocket or with a poll-based system, using
//! the HTTP client.
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::{Arc, RwLock};

use cdk_common::nut17::NotificationId;
use cdk_common::pub_sub::remote_consumer::{
    Consumer, MessageToTransportLoop, RemoteActiveConsumer, Transport,
};
use cdk_common::pub_sub::{Error as PubsubError, Event, Pubsub, Topic};
use cdk_common::subscription::{Params, WalletParams};
use cdk_common::CheckStateRequest;
use tokio::sync::mpsc;
use uuid::Uuid;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_futures;

use crate::event::MintEvent;
use crate::mint_url::MintUrl;
use crate::wallet::MintConnector;

type WsSubscriptionBody = (mpsc::Sender<NotificationPayload>, Params);
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
}

#[async_trait::async_trait]
impl Transport for SubscriptionClient {
    type Topic = MintSubTopics;

    fn new_name(&self) -> <Self::Topic as Topic>::SubscriptionName {
        Uuid::new_v4().to_string()
    }

    async fn long_connection(
        &self,
        _subscribe_changes: mpsc::Receiver<MessageToTransportLoop<Self::Topic>>,
        _topics: Vec<<<Self::Topic as Topic>::Event as Event>::Topic>,
    ) -> Result<(), PubsubError>
    where
        Self: Sized,
    {
        Err(cdk_common::pub_sub::Error::NotSupported)
    }

    /// Poll on demand
    async fn poll(
        &self,
        topics: Vec<<<Self::Topic as Topic>::Event as Event>::Topic>,
        reply_to: Arc<Pubsub<Self::Topic>>,
    ) -> Result<(), PubsubError> {
        let proofs = topics
            .iter()
            .filter_map(|x| match &x {
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
                        NotificationPayload::MeltQuoteBolt11Response(response.into()),
                    ));
                }
                _ => {}
            }
        }

        Ok(())
    }
}

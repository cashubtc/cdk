//! Client for subscriptions
//!
//! Mint servers can send notifications to clients about changes in the state,
//! according to NUT-17, using the WebSocket protocol. This module provides a
//! subscription manager that allows clients to subscribe to notifications from
//! multiple mint servers using WebSocket or with a poll-based system, using
//! the HTTP client.
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;

use tokio::sync::{mpsc, RwLock};
use tokio::task::JoinHandle;
use tracing::error;

use crate::mint_url::MintUrl;
use crate::nuts::nut17::Params;
use crate::pub_sub::SubId;
use crate::wallet::client::MintConnector;

mod http;
#[cfg(all(
    not(feature = "http_subscription"),
    feature = "mint",
    not(target_arch = "wasm32")
))]
mod ws;

type WsSubscriptionBody = (mpsc::Sender<NotificationPayload>, Params);

/// Subscription manager
///
/// This structure should be instantiated once per wallet at most. It is
/// clonable since all its members are Arcs.
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
#[derive(Debug, Clone)]
pub struct SubscriptionManager {
    all_connections: Arc<RwLock<HashMap<MintUrl, SubscriptionClient>>>,
    http_client: Arc<dyn MintConnector + Send + Sync>,
}

impl SubscriptionManager {
    /// Create a new subscription manager
    pub fn new(http_client: Arc<dyn MintConnector + Send + Sync>) -> Self {
        Self {
            all_connections: Arc::new(RwLock::new(HashMap::new())),
            http_client,
        }
    }

    /// Subscribe to updates from a mint server with a given filter
    pub async fn subscribe(&self, mint_url: MintUrl, filter: Params) -> ActiveSubscription {
        let subscription_clients = self.all_connections.read().await;
        let id = filter.id.clone();
        if let Some(subscription_client) = subscription_clients.get(&mint_url) {
            let (on_drop_notif, receiver) = subscription_client.subscribe(filter).await;
            ActiveSubscription::new(receiver, id, on_drop_notif)
        } else {
            drop(subscription_clients);

            #[cfg(all(
                not(feature = "http_subscription"),
                feature = "mint",
                not(target_arch = "wasm32")
            ))]
            let is_ws_support = self
                .http_client
                .get_mint_info()
                .await
                .map(|info| !info.nuts.nut17.supported.is_empty())
                .unwrap_or_default();

            #[cfg(any(
                feature = "http_subscription",
                not(feature = "mint"),
                target_arch = "wasm32"
            ))]
            let is_ws_support = false;

            tracing::debug!(
                "Connect to {:?} to subscribe. WebSocket is supported ({})",
                mint_url,
                is_ws_support
            );

            let mut subscription_clients = self.all_connections.write().await;
            let subscription_client =
                SubscriptionClient::new(mint_url.clone(), self.http_client.clone(), is_ws_support);
            let (on_drop_notif, receiver) = subscription_client.subscribe(filter).await;
            subscription_clients.insert(mint_url, subscription_client);

            ActiveSubscription::new(receiver, id, on_drop_notif)
        }
    }
}

/// Subscription client
///
/// If the server supports WebSocket subscriptions, this client will be used,
/// otherwise the HTTP pool and pause will be used (which is the less efficient
/// method).
#[derive(Debug)]
pub struct SubscriptionClient {
    new_subscription_notif: mpsc::Sender<SubId>,
    on_drop_notif: mpsc::Sender<SubId>,
    subscriptions: Arc<RwLock<HashMap<SubId, WsSubscriptionBody>>>,
    worker: Option<JoinHandle<()>>,
}

type NotificationPayload = crate::nuts::NotificationPayload<String>;

/// Active Subscription
pub struct ActiveSubscription {
    sub_id: Option<SubId>,
    on_drop_notif: mpsc::Sender<SubId>,
    receiver: mpsc::Receiver<NotificationPayload>,
}

impl ActiveSubscription {
    fn new(
        receiver: mpsc::Receiver<NotificationPayload>,
        sub_id: SubId,
        on_drop_notif: mpsc::Sender<SubId>,
    ) -> Self {
        Self {
            sub_id: Some(sub_id),
            on_drop_notif,
            receiver,
        }
    }

    /// Try to receive a notification
    pub fn try_recv(&mut self) -> Result<Option<NotificationPayload>, Error> {
        match self.receiver.try_recv() {
            Ok(payload) => Ok(Some(payload)),
            Err(mpsc::error::TryRecvError::Empty) => Ok(None),
            Err(mpsc::error::TryRecvError::Disconnected) => Err(Error::Disconnected),
        }
    }

    /// Receive a notification asynchronously
    pub async fn recv(&mut self) -> Option<NotificationPayload> {
        self.receiver.recv().await
    }
}

impl Drop for ActiveSubscription {
    fn drop(&mut self) {
        if let Some(sub_id) = self.sub_id.take() {
            let _ = self.on_drop_notif.try_send(sub_id);
        }
    }
}

/// Subscription client error
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// Url error
    #[error("Could not join paths: {0}")]
    Url(#[from] crate::mint_url::Error),
    /// Disconnected from the notification channel
    #[error("Disconnected from the notification channel")]
    Disconnected,
}

impl SubscriptionClient {
    /// Create new [`WebSocketClient`]
    pub fn new(
        url: MintUrl,
        http_client: Arc<dyn MintConnector + Send + Sync>,
        prefer_ws_method: bool,
    ) -> Self {
        let subscriptions = Arc::new(RwLock::new(HashMap::new()));
        let (new_subscription_notif, new_subscription_recv) = mpsc::channel(100);
        let (on_drop_notif, on_drop_recv) = mpsc::channel(1000);

        Self {
            new_subscription_notif,
            on_drop_notif,
            subscriptions: subscriptions.clone(),
            worker: Some(Self::start_worker(
                prefer_ws_method,
                http_client,
                url,
                subscriptions,
                new_subscription_recv,
                on_drop_recv,
            )),
        }
    }

    #[allow(unused_variables)]
    fn start_worker(
        prefer_ws_method: bool,
        http_client: Arc<dyn MintConnector + Send + Sync>,
        url: MintUrl,
        subscriptions: Arc<RwLock<HashMap<SubId, WsSubscriptionBody>>>,
        new_subscription_recv: mpsc::Receiver<SubId>,
        on_drop_recv: mpsc::Receiver<SubId>,
    ) -> JoinHandle<()> {
        #[cfg(any(
            feature = "http_subscription",
            not(feature = "mint"),
            target_arch = "wasm32"
        ))]
        return Self::http_worker(
            http_client,
            url,
            subscriptions,
            new_subscription_recv,
            on_drop_recv,
        );

        #[cfg(all(
            not(feature = "http_subscription"),
            feature = "mint",
            not(target_arch = "wasm32")
        ))]
        if prefer_ws_method {
            Self::ws_worker(
                http_client,
                url,
                subscriptions,
                new_subscription_recv,
                on_drop_recv,
            )
        } else {
            Self::http_worker(
                http_client,
                subscriptions,
                new_subscription_recv,
                on_drop_recv,
            )
        }
    }

    /// Subscribe to a WebSocket channel
    pub async fn subscribe(
        &self,
        filter: Params,
    ) -> (mpsc::Sender<SubId>, mpsc::Receiver<NotificationPayload>) {
        let mut subscriptions = self.subscriptions.write().await;
        let id = filter.id.clone();

        let (sender, receiver) = mpsc::channel(10_000);
        subscriptions.insert(id.clone(), (sender, filter));
        drop(subscriptions);

        let _ = self.new_subscription_notif.send(id).await;
        (self.on_drop_notif.clone(), receiver)
    }

    /// HTTP subscription client
    ///
    /// This is a poll based subscription, where the client will poll the server
    /// from time to time to get updates, notifying the subscribers on changes
    fn http_worker(
        http_client: Arc<dyn MintConnector + Send + Sync>,
        subscriptions: Arc<RwLock<HashMap<SubId, WsSubscriptionBody>>>,
        new_subscription_recv: mpsc::Receiver<SubId>,
        on_drop: mpsc::Receiver<SubId>,
    ) -> JoinHandle<()> {
        let http_worker = http::http_main(
            vec![],
            http_client,
            subscriptions,
            new_subscription_recv,
            on_drop,
        );

        #[cfg(target_arch = "wasm32")]
        let ret = tokio::task::spawn_local(http_worker);

        #[cfg(not(target_arch = "wasm32"))]
        let ret = tokio::spawn(http_worker);

        ret
    }

    /// WebSocket subscription client
    ///
    /// This is a WebSocket based subscription, where the client will connect to
    /// the server and stay there idle waiting for server-side notifications
    #[allow(clippy::incompatible_msrv)]
    #[cfg(all(
        not(feature = "http_subscription"),
        feature = "mint",
        not(target_arch = "wasm32")
    ))]
    fn ws_worker(
        http_client: Arc<dyn MintConnector + Send + Sync>,
        url: MintUrl,
        subscriptions: Arc<RwLock<HashMap<SubId, WsSubscriptionBody>>>,
        new_subscription_recv: mpsc::Receiver<SubId>,
        on_drop: mpsc::Receiver<SubId>,
    ) -> JoinHandle<()> {
        tokio::spawn(ws::ws_main(
            http_client,
            url,
            subscriptions,
            new_subscription_recv,
            on_drop,
        ))
    }
}

impl Drop for SubscriptionClient {
    fn drop(&mut self) {
        if let Some(sender) = self.worker.take() {
            sender.abort();
        }
    }
}

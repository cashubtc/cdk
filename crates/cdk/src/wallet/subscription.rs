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

use cdk_common::nut17::ws::{
    WsErrorResponse, WsMethodRequest, WsNotification, WsRequest, WsResponse, WsUnsubscribeRequest,
};
use cdk_common::nut17::{Kind, NotificationId};
use cdk_common::parking_lot::RwLock;
use cdk_common::pub_sub::remote_consumer::{
    Consumer, InternalRelay, RemoteActiveConsumer, StreamCtrl, SubscribeMessage, Transport,
};
use cdk_common::pub_sub::{Error as PubsubError, Spec, Subscriber};
use cdk_common::subscription::WalletParams;
use cdk_common::ws_client::{connect as ws_connect, WsError};
use cdk_common::{
    CheckStateRequest, MeltQuoteBolt11Response, MeltQuoteBolt12Response, MeltQuoteCustomResponse,
    Method, MintQuoteBolt11Response, MintQuoteBolt12Response, MintQuoteCustomResponse,
    PaymentMethod, ProofState, RoutePath,
};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::event::MintEvent;
use crate::mint_url::MintUrl;
use crate::wallet::MintConnector;

#[derive(Debug, Clone, serde::Deserialize)]
struct RawNotificationInner<I> {
    #[serde(rename = "subId")]
    sub_id: I,
    payload: serde_json::Value,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(bound = "I: serde::Serialize + serde::de::DeserializeOwned")]
#[serde(untagged)]
enum RawWsMessageOrResponse<I> {
    Response(WsResponse<I>),
    ErrorResponse(WsErrorResponse),
    Notification(Box<WsNotification<RawNotificationInner<I>>>),
}

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

fn decode_notification_payload(
    kind: &Kind,
    payload: serde_json::Value,
) -> Result<NotificationPayload, PubsubError> {
    match kind {
        Kind::ProofState => serde_json::from_value::<ProofState>(payload)
            .map(NotificationPayload::ProofState)
            .map_err(|err| PubsubError::ParsingError(err.to_string())),
        Kind::Bolt11MintQuote => serde_json::from_value::<MintQuoteBolt11Response<String>>(payload)
            .map(NotificationPayload::MintQuoteBolt11Response)
            .map_err(|err| PubsubError::ParsingError(err.to_string())),
        Kind::Bolt11MeltQuote => serde_json::from_value::<MeltQuoteBolt11Response<String>>(payload)
            .map(NotificationPayload::MeltQuoteBolt11Response)
            .map_err(|err| PubsubError::ParsingError(err.to_string())),
        Kind::Bolt12MintQuote => serde_json::from_value::<MintQuoteBolt12Response<String>>(payload)
            .map(NotificationPayload::MintQuoteBolt12Response)
            .map_err(|err| PubsubError::ParsingError(err.to_string())),
        Kind::Bolt12MeltQuote => serde_json::from_value::<MeltQuoteBolt12Response<String>>(payload)
            .map(NotificationPayload::MeltQuoteBolt12Response)
            .map_err(|err| PubsubError::ParsingError(err.to_string())),
        Kind::Custom(method) if method.ends_with("_mint_quote") => serde_json::from_value::<
            MintQuoteCustomResponse<String>,
        >(payload)
        .map(|response| NotificationPayload::CustomMintQuoteResponse(method.clone(), response))
        .map_err(|err| PubsubError::ParsingError(err.to_string())),
        Kind::Custom(method) if method.ends_with("_melt_quote") => serde_json::from_value::<
            MeltQuoteCustomResponse<String>,
        >(payload)
        .map(|response| NotificationPayload::CustomMeltQuoteResponse(method.clone(), response))
        .map_err(|err| PubsubError::ParsingError(err.to_string())),
        Kind::Custom(method) => Err(PubsubError::ParsingError(format!(
            "Unsupported custom websocket notification kind: {method}"
        ))),
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

async fn stream_client(
    client: &SubscriptionClient,
    mut ctrl: mpsc::Receiver<StreamCtrl<MintSubTopics>>,
    topics: Vec<SubscribeMessage<MintSubTopics>>,
    reply_to: InternalRelay<MintSubTopics>,
) -> Result<(), PubsubError> {
    let mut sub_id_to_kind = HashMap::new();

    let mut url = client
        .mint_url
        .join_paths(&["v1", "ws"])
        .expect("Could not join paths");

    if url.scheme() == "https" {
        url.set_scheme("wss").expect("Could not set scheme");
    } else {
        url.set_scheme("ws").expect("Could not set scheme");
    }

    let mut headers: Vec<(&str, String)> = Vec::new();

    {
        let auth_wallet = client.http_client.get_auth_wallet().await;
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

            let header_value = auth_token.to_string();
            headers.push((header_key, header_value));
        }
    }

    let url_str = url.to_string();
    let header_refs: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (*k, v.as_str())).collect();

    tracing::debug!("Connecting to {}", url);
    let (mut sender, mut receiver) = ws_connect(&url_str, &header_refs).await.map_err(|err| {
        tracing::error!("Error connecting: {err:?}");
        map_ws_error(err)
    })?;

    tracing::debug!("Connected to {}", url);

    for (name, index) in topics {
        let kind = SubscriptionClient::subscription_kind(&index);
        let (_, req) = if let Some(req) = client.get_sub_request(name.clone(), index) {
            req
        } else {
            continue;
        };

        sub_id_to_kind.insert(name, kind);
        let _ = sender.send(req).await;
    }

    loop {
        tokio::select! {
            Some(msg) = ctrl.recv() => {
                match msg {
                    StreamCtrl::Subscribe(msg) => {
                        let kind = SubscriptionClient::subscription_kind(&msg.1);
                        let (_, req) = if let Some(req) = client.get_sub_request(msg.0.clone(), msg.1) {
                            req
                        } else {
                            continue;
                        };
                        sub_id_to_kind.insert(msg.0, kind);
                        let _ = sender.send(req).await;
                    }
                    StreamCtrl::Unsubscribe(msg) => {
                        sub_id_to_kind.remove(&msg);
                        let req = if let Some(req) = client.get_unsub_request(msg) {
                            req
                        } else {
                            continue;
                        };
                        let _ = sender.send(req).await;
                    }
                    StreamCtrl::Stop => {
                        if let Err(err) = sender.close().await {
                            tracing::error!("Closing error {err:?}");
                        }
                        break;
                    }
                };
            }
            msg = receiver.recv() => {
                let msg = match msg {
                    Some(Ok(msg)) => msg,
                    Some(Err(_)) => {
                        if let Err(err) = sender.close().await {
                            tracing::error!("Closing error {err:?}");
                        }
                        sub_id_to_kind.clear();
                        break;
                    }
                    None => {
                        sub_id_to_kind.clear();
                        break;
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

                        let payload = decode_notification_payload(
                            kind,
                            payload.params.payload.clone(),
                        )?;
                        reply_to.send(payload);
                    }
                    RawWsMessageOrResponse::Response(response) => {
                        tracing::debug!("Received response from server: {:?}", response);
                    }
                    RawWsMessageOrResponse::ErrorResponse(error) => {
                        tracing::debug!("Received an error from server: {:?}", error);
                        return Err(PubsubError::InternalStr(error.error.message));
                    }
                }
            }
        }
    }

    Ok(())
}

fn map_ws_error(err: WsError) -> PubsubError {
    match err {
        WsError::Connection(_) => PubsubError::NotSupported,
        other => PubsubError::InternalStr(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn decode_proof_state_notification() {
        let payload = json!({
            "Y": "02194603ffa062682c4f10e2dfe8f53e17d5d0329db51c8d3935cc74a4c0e0d4cb",
            "state": "UNSPENT",
            "witness": null
        });

        let decoded = decode_notification_payload(&Kind::ProofState, payload).unwrap();

        assert!(matches!(decoded, NotificationPayload::ProofState(_)));
    }

    #[test]
    fn decode_bolt11_notifications() {
        let mint_payload = json!({
            "quote": "mint-quote",
            "request": "lnbc1...",
            "state": "PAID",
            "expiry": 1234,
            "paid": true
        });
        let melt_payload = json!({
            "quote": "melt-quote",
            "amount": 21,
            "fee_reserve": 1,
            "state": "PAID",
            "expiry": 1234,
            "payment_proof": "abc"
        });

        let mint_decoded =
            decode_notification_payload(&Kind::Bolt11MintQuote, mint_payload).unwrap();
        let melt_decoded =
            decode_notification_payload(&Kind::Bolt11MeltQuote, melt_payload).unwrap();

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
        let payload = json!({
            "quote": "quote-id",
            "request": "lni1...",
            "amount": null,
            "unit": "sat",
            "state": "UNPAID",
            "expiry": 1234,
            "pubkey": "02194603ffa062682c4f10e2dfe8f53e17d5d0329db51c8d3935cc74a4c0e0d4cb",
            "amount_paid": 0,
            "amount_issued": 0
        });

        let decoded = decode_notification_payload(&Kind::Bolt12MintQuote, payload).unwrap();

        assert!(matches!(
            decoded,
            NotificationPayload::MintQuoteBolt12Response(_)
        ));
    }

    #[test]
    fn decode_custom_notifications() {
        let mint_method = "foo_mint_quote".to_string();
        let melt_method = "foo_melt_quote".to_string();
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
            decode_notification_payload(&Kind::Custom(mint_method.clone()), mint_payload).unwrap();
        let melt_decoded =
            decode_notification_payload(&Kind::Custom(melt_method.clone()), melt_payload).unwrap();

        assert!(matches!(
            mint_decoded,
            NotificationPayload::CustomMintQuoteResponse(method, _) if method == mint_method
        ));
        assert!(matches!(
            melt_decoded,
            NotificationPayload::CustomMeltQuoteResponse(method, _) if method == melt_method
        ));
    }

    #[test]
    fn decode_unknown_custom_kind_errors() {
        let err = decode_notification_payload(&Kind::Custom("foo_status".to_string()), json!({}))
            .unwrap_err();

        assert!(matches!(err, PubsubError::ParsingError(_)));
    }

    #[test]
    fn decode_wrong_kind_errors() {
        let payload = json!({
            "Y": "02194603ffa062682c4f10e2dfe8f53e17d5d0329db51c8d3935cc74a4c0e0d4cb",
            "state": "UNSPENT",
            "witness": null
        });

        let err = decode_notification_payload(&Kind::Bolt12MintQuote, payload).unwrap_err();

        assert!(matches!(err, PubsubError::ParsingError(_)));
    }
}

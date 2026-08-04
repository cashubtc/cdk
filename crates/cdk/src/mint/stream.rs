//! Transport-agnostic NUT-17 subscription runner.
//!
//! [`run_stream`] drives the NUT-17 subscribe/notify protocol over a raw
//! [`StreamTx`] / [`StreamRx`] duplex against a [`Mint`], independent of how the
//! stream is carried. `MintServer::open_stream` uses it over an in-memory duplex;
//! a network transport (its adapter) can bridge an accepted socket to the same
//! halves and reuse it.

use std::collections::HashMap;
use std::sync::Arc;

use cdk_common::nut17::NotificationPayload;
use cdk_common::stream_channel::{StreamRx, StreamTx};
use cdk_common::subscription::SubId;
use cdk_common::ws::{
    notification_to_ws_message, NotificationInner, WsErrorBody, WsMessageOrResponse,
    WsMethodRequest, WsRequest, WsResponseResult,
};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio::task::JoinHandle;

use super::{Mint, QuoteId};

const MAX_SUBSCRIPTIONS_PER_CONNECTION: usize = 100;
const MAX_FILTERS_PER_SUBSCRIPTION: usize = 1000;

impl Mint {
    /// Run the NUT-17 subscription protocol over a caller-provided stream until
    /// it closes.
    ///
    /// A transport adapter that already owns a bidirectional stream (a QUIC
    /// stream for Iroh, a Noise sub-stream for an enclave, an accepted
    /// WebSocket) wraps it into [`StreamTx`]/[`StreamRx`] and hands it here,
    /// instead of using [`open_stream`](crate::mint::MintServer::open_stream),
    /// which is for the in-process case.
    ///
    /// Performs no authentication: the adapter must gate the stream with
    /// [`verify_auth`](Mint::verify_auth) before calling this, the way the
    /// `cdk-axum` websocket handler does.
    pub async fn serve_stream(&self, tx: StreamTx, rx: StreamRx) {
        run_stream(self.clone(), tx, rx).await;
    }
}

/// The pump tasks feeding this connection's subscriptions.
///
/// A dedicated type so its [`Drop`] aborts every task on any exit path,
/// including an unwind. Draining at the end of the run loop would leak the
/// tasks on a panic, because `JoinHandle`'s own drop detaches rather than
/// aborts.
#[derive(Default)]
struct Subscriptions {
    handles: HashMap<Arc<SubId>, JoinHandle<()>>,
}

impl Subscriptions {
    fn contains(&self, sub_id: &Arc<SubId>) -> bool {
        self.handles.contains_key(sub_id)
    }

    fn len(&self) -> usize {
        self.handles.len()
    }

    fn insert(&mut self, sub_id: Arc<SubId>, handle: JoinHandle<()>) {
        self.handles.insert(sub_id, handle);
    }

    fn remove(&mut self, sub_id: &Arc<SubId>) -> Option<JoinHandle<()>> {
        self.handles.remove(sub_id)
    }
}

impl Drop for Subscriptions {
    fn drop(&mut self) {
        for (_, handle) in self.handles.drain() {
            handle.abort();
        }
    }
}

/// Run the NUT-17 protocol over one duplex stream until it closes.
pub(super) async fn run_stream(mint: Mint, mut tx: StreamTx, mut rx: StreamRx) {
    let (publisher, mut subscriber) =
        mpsc::channel::<(Arc<SubId>, NotificationPayload<QuoteId>)>(100);
    let mut subscriptions = Subscriptions::default();

    loop {
        tokio::select! {
            Some((sub_id, payload)) = subscriber.recv() => {
                if !subscriptions.contains(&sub_id) {
                    // The subscription was dropped but a queued notification
                    // arrived before its pump task stopped; ignore it.
                    continue;
                }
                let notification = notification_to_ws_message(NotificationInner { sub_id, payload });
                let message = match serde_json::to_string(&notification) {
                    Ok(message) => message,
                    Err(err) => {
                        tracing::error!("Could not serialize ws notification: {err}");
                        continue;
                    }
                };
                if tx.send(message).await.is_err() {
                    break;
                }
            }
            incoming = rx.recv() => {
                let text = match incoming {
                    Some(Ok(text)) => text,
                    Some(Err(err)) => {
                        tracing::warn!("Stream receive error: {err}");
                        break;
                    }
                    None => break,
                };
                let (id, result) = match deserialize_request(&text) {
                    Ok(request) => {
                        let id = request.id;
                        (
                            id,
                            handle_request(&mint, &publisher, &mut subscriptions, request).await,
                        )
                    }
                    Err((err, id)) => {
                        tracing::error!("Could not parse ws request: {err}");
                        match id {
                            Some(id) => (id, Err(invalid_params())),
                            None => continue,
                        }
                    }
                };
                let response: WsMessageOrResponse = (id, result).into();
                let message = match serde_json::to_string(&response) {
                    Ok(message) => message,
                    Err(err) => {
                        tracing::error!("Could not serialize ws response: {err}");
                        continue;
                    }
                };
                if tx.send(message).await.is_err() {
                    break;
                }
            }
            else => break,
        }
    }
    // `subscriptions` drops here (or on unwind), aborting every pump task.
}

/// Parse a request, recovering its `id` when the payload is valid JSON but not a
/// valid request, so a rejected frame (an oversized `subId` or `kind`) can be
/// answered with an error carrying the client's id instead of being dropped.
fn deserialize_request(text: &str) -> Result<WsRequest, (serde_json::Error, Option<usize>)> {
    let value = serde_json::from_str::<serde_json::Value>(text).map_err(|err| (err, None))?;
    let id = value
        .get("id")
        .and_then(serde_json::Value::as_u64)
        .and_then(|id| usize::try_from(id).ok());

    serde_json::from_value(value).map_err(|err| (err, id))
}

async fn handle_request(
    mint: &Mint,
    publisher: &mpsc::Sender<(Arc<SubId>, NotificationPayload<QuoteId>)>,
    subscriptions: &mut Subscriptions,
    request: WsRequest,
) -> Result<WsResponseResult, WsErrorBody> {
    match request.method {
        WsMethodRequest::Subscribe(params) => {
            let sub_id = params.id.clone();
            if subscriptions.contains(&sub_id) {
                return Err(invalid_params());
            }
            if subscriptions.len() >= MAX_SUBSCRIPTIONS_PER_CONNECTION {
                tracing::warn!(
                    "subscription request exceeds per-connection limit: {} >= {}",
                    subscriptions.len(),
                    MAX_SUBSCRIPTIONS_PER_CONNECTION
                );
                return Err(invalid_params());
            }
            if params.filters.len() > MAX_FILTERS_PER_SUBSCRIPTION {
                tracing::warn!(
                    "subscription request exceeds max filters limit: {} > {}",
                    params.filters.len(),
                    MAX_FILTERS_PER_SUBSCRIPTION
                );
                return Err(invalid_params());
            }

            let mut subscription = mint.pubsub_manager().subscribe(params).map_err(|err| {
                tracing::error!("Could not subscribe: {err}");
                internal_error()
            })?;

            let publisher = publisher.clone();
            let sub_id_for_sender = sub_id.clone();
            subscriptions.insert(
                sub_id.clone(),
                tokio::spawn(async move {
                    while let Some(event) = subscription.recv().await {
                        // The publisher channel is bounded (100) and shared by
                        // every subscription on this connection, so a burst
                        // drops notifications rather than blocking. The wallet's
                        // HTTP poll fallback is the safety net.
                        match publisher.try_send((sub_id_for_sender.clone(), event.into_inner())) {
                            Ok(()) => {}
                            Err(TrySendError::Full(_)) => tracing::warn!(
                                "Dropping notification for {sub_id_for_sender:?}: publisher is full"
                            ),
                            Err(TrySendError::Closed(_)) => break,
                        }
                    }
                }),
            );

            Ok(WsResponseResult {
                status: "OK".to_string(),
                sub_id,
            })
        }
        WsMethodRequest::Unsubscribe(req) => match subscriptions.remove(&req.sub_id) {
            Some(handle) => {
                handle.abort();
                Ok(WsResponseResult {
                    status: "OK".to_string(),
                    sub_id: req.sub_id,
                })
            }
            None => Err(invalid_params()),
        },
    }
}

fn invalid_params() -> WsErrorBody {
    WsErrorBody {
        code: -32602,
        message: "Invalid params".to_string(),
    }
}

fn internal_error() -> WsErrorBody {
    WsErrorBody {
        code: -32603,
        message: "Internal error".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use cdk_common::nut17::{Kind, MAX_CUSTOM_KIND_LEN, MAX_SUBSCRIPTION_ID_LEN};
    use cdk_common::stream_channel::in_memory_pair;
    use cdk_common::subscription::Params;
    use cdk_common::ws::WsUnsubscribeRequest;
    use tokio::time::timeout;

    use super::*;
    use crate::test_helpers::mint::{create_test_mint, create_test_mint_with_limits};

    fn subscribe_frame(sub_id: &str, filters: usize) -> String {
        let params = Params {
            kind: Kind::Bolt11MintQuote,
            filters: (0..filters).map(|_| QuoteId::new().to_string()).collect(),
            id: Arc::new(SubId::from(sub_id)),
        };
        serde_json::to_string(&WsRequest::from((WsMethodRequest::Subscribe(params), 0))).unwrap()
    }

    fn unsubscribe_frame(sub_id: &str) -> String {
        let req = WsUnsubscribeRequest {
            sub_id: Arc::new(SubId::from(sub_id)),
        };
        serde_json::to_string(&WsRequest::from((WsMethodRequest::Unsubscribe(req), 1))).unwrap()
    }

    /// Poll until the mint reports `expected` active subscribers, or fail. The
    /// pump task registers the subscription asynchronously, so a count assert
    /// needs to wait rather than read once.
    async fn wait_for_subscribers(mint: &Mint, expected: usize) {
        let pubsub = mint.pubsub_manager();
        let settled = timeout(Duration::from_secs(2), async {
            while pubsub.active_subscribers() != expected {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        assert!(
            settled.is_ok(),
            "expected {expected} active subscribers, got {}",
            pubsub.active_subscribers()
        );
    }

    /// Spawn `run_stream` over the server half of an in-memory duplex and return
    /// the client half plus the runner handle.
    fn spawn_runner(mint: &Mint) -> ((StreamTx, StreamRx), JoinHandle<()>) {
        let (client, (server_tx, server_rx)) = in_memory_pair();
        let mint = mint.clone();
        let runner = tokio::spawn(async move { run_stream(mint, server_tx, server_rx).await });
        (client, runner)
    }

    async fn expect_reply(rx: &mut StreamRx) -> String {
        rx.recv()
            .await
            .expect("stream still open")
            .expect("a reply")
    }

    #[test]
    fn oversized_subscription_fields_recover_the_request_id() {
        for request in [
            serde_json::json!({
                "jsonrpc": "2.0",
                "method": "subscribe",
                "params": {
                    "kind": "bolt11_mint_quote",
                    "filters": [],
                    "subId": "a".repeat(MAX_SUBSCRIPTION_ID_LEN + 1),
                },
                "id": 1,
            }),
            serde_json::json!({
                "jsonrpc": "2.0",
                "method": "subscribe",
                "params": {
                    "kind": "a".repeat(MAX_CUSTOM_KIND_LEN + 1),
                    "filters": [],
                    "subId": "subscription",
                },
                "id": 2,
            }),
            serde_json::json!({
                "jsonrpc": "2.0",
                "method": "unsubscribe",
                "params": {
                    "subId": "a".repeat(MAX_SUBSCRIPTION_ID_LEN + 1),
                },
                "id": 3,
            }),
        ] {
            let (err, id) =
                deserialize_request(&request.to_string()).expect_err("oversized request");
            assert!(
                err.to_string().contains("exceeds"),
                "unexpected error: {err}"
            );

            let response: WsMessageOrResponse =
                (id.expect("request ID"), Err(invalid_params())).into();
            let response = serde_json::to_value(response).expect("error response");
            assert_eq!(response["error"]["code"], serde_json::json!(-32602));
            assert_eq!(
                response["error"]["message"],
                serde_json::json!("Invalid params")
            );
            assert_eq!(response["jsonrpc"], serde_json::json!("2.0"));
            assert_eq!(response["id"], request["id"]);
        }
    }

    #[tokio::test]
    async fn unsubscribe_cleans_up_subscription() {
        let mint = create_test_mint().await.unwrap();
        let base = mint.pubsub_manager().active_subscribers();
        let ((mut tx, mut rx), runner) = spawn_runner(&mint);

        tx.send(subscribe_frame("sub-1", 1)).await.unwrap();
        assert!(expect_reply(&mut rx).await.contains("OK"));
        wait_for_subscribers(&mint, base + 1).await;

        tx.send(unsubscribe_frame("sub-1")).await.unwrap();
        assert!(expect_reply(&mut rx).await.contains("OK"));
        wait_for_subscribers(&mint, base).await;

        drop(tx);
        let _ = runner.await;
    }

    /// A client disconnect (dropping both halves) must abort every pump task via
    /// the `Subscriptions` guard, not leak them.
    #[tokio::test]
    async fn disconnect_cleans_up_subscriptions() {
        let mint = create_test_mint().await.unwrap();
        let base = mint.pubsub_manager().active_subscribers();
        let ((mut tx, mut rx), runner) = spawn_runner(&mint);

        for id in ["sub-A", "sub-B"] {
            tx.send(subscribe_frame(id, 1)).await.unwrap();
            assert!(expect_reply(&mut rx).await.contains("OK"));
        }
        wait_for_subscribers(&mint, base + 2).await;

        drop(tx);
        drop(rx);
        let _ = timeout(Duration::from_secs(2), runner).await;
        wait_for_subscribers(&mint, base).await;
    }

    #[tokio::test]
    async fn per_connection_subscription_cap() {
        let mint = create_test_mint().await.unwrap();
        let base = mint.pubsub_manager().active_subscribers();
        let ((mut tx, mut rx), runner) = spawn_runner(&mint);

        for i in 0..MAX_SUBSCRIPTIONS_PER_CONNECTION {
            tx.send(subscribe_frame(&format!("sub-{i}"), 1))
                .await
                .unwrap();
            assert!(
                expect_reply(&mut rx).await.contains("OK"),
                "sub {i} not acked"
            );
        }
        wait_for_subscribers(&mint, base + MAX_SUBSCRIPTIONS_PER_CONNECTION).await;

        // One over the cap is rejected and allocates no pub/sub subscriber.
        tx.send(subscribe_frame("sub-over", 1)).await.unwrap();
        let reply = expect_reply(&mut rx).await;
        assert!(
            reply.contains("Invalid params"),
            "over-cap not rejected: {reply}"
        );
        wait_for_subscribers(&mint, base + MAX_SUBSCRIPTIONS_PER_CONNECTION).await;

        drop(tx);
        let _ = runner.await;
    }

    /// The filter cap is `MAX_FILTERS_PER_SUBSCRIPTION`, independent of the
    /// mint's swap input/output limits: five filters on a limit-2 mint is fine.
    #[tokio::test]
    async fn filter_count_not_tied_to_max_inputs() {
        let mint = create_test_mint_with_limits(2, 2).await.unwrap();
        let ((mut tx, mut rx), runner) = spawn_runner(&mint);

        tx.send(subscribe_frame("many-filters", 5)).await.unwrap();
        let reply = expect_reply(&mut rx).await;
        assert!(
            reply.contains("OK"),
            "5 filters should be accepted: {reply}"
        );

        drop(tx);
        let _ = runner.await;
    }
}

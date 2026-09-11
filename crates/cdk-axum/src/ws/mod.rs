use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use axum::extract::ws::{close_code, CloseFrame, Message, WebSocket};
use cdk::mint::QuoteId;
use cdk::nuts::nut17::NotificationPayload;
use cdk::subscription::SubId;
use cdk::ws::{
    notification_to_ws_message, NotificationInner, WsErrorBody, WsMessageOrResponse,
    WsMethodRequest, WsRequest,
};
use cdk_common::terminal::escape_control;
use futures::StreamExt;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use crate::MintState;

mod budget;
mod error;
mod limits;
mod subscribe;
mod unsubscribe;

pub(crate) use budget::{Charge, RequestBudget};
pub use limits::WsLimits;
pub(crate) use limits::{WsConnectionGuard, WsConnectionLimiter, WsRejection};

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

/// A frame that could not be parsed, carrying the JSON-RPC id when one could
/// still be recovered so the client can correlate the error.
#[derive(Debug)]
struct ParseFailure {
    error: serde_json::Error,
    id: Option<usize>,
}

/// Only the JSON-RPC id, used to answer a frame whose body was never parsed.
#[derive(Deserialize)]
struct RequestEnvelope {
    #[serde(default)]
    id: Option<usize>,
}

fn deserialize_request(text: &str) -> Result<WsRequest, ParseFailure> {
    serde_json::from_str::<WsRequest>(text).map_err(|error| ParseFailure {
        id: recover_request_id(text),
        error,
    })
}

/// Recovers the request id while skipping every other value, so a frame that is
/// unparsable or over the rate limit is never walked as a full
/// `serde_json::Value`.
fn recover_request_id(text: &str) -> Option<usize> {
    match serde_json::from_str::<RequestEnvelope>(text) {
        Ok(envelope) => envelope.id,
        Err(_) => None,
    }
}

fn error_response(
    request_id: usize,
    error: WsError,
) -> Result<serde_json::Value, serde_json::Error> {
    let response: WsMessageOrResponse = (request_id, Err(WsErrorBody::from(error))).into();
    serde_json::to_value(response)
}

pub use error::WsError;

/// One live subscription, with the share of the connection's topic budget it
/// claimed so that unsubscribing can give exactly that much back.
struct SubscriptionSlot {
    handle: JoinHandle<()>,
    topics: usize,
}

pub struct WsContext {
    state: MintState,
    subscriptions: HashMap<Arc<SubId>, SubscriptionSlot>,
    topics_in_use: usize,
    budget: RequestBudget,
    publisher: mpsc::Sender<(Arc<SubId>, NotificationPayload<QuoteId>)>,
    /// Declared last so the manual `Drop` below tears down the subscriptions
    /// before the connection slot is handed back.
    _connection_guard: WsConnectionGuard,
}

impl Drop for WsContext {
    fn drop(&mut self) {
        for (_, slot) in self.subscriptions.drain() {
            slot.handle.abort();
        }
    }
}

/// Main function for websocket connections
///
/// This function will handle all incoming websocket connections and keep them in their own loop.
///
/// For simplicity sake this function will spawn tasks for each subscription and
/// keep them in a hashmap, and will have a single subscriber for all of them.
pub async fn main_websocket(
    mut socket: WebSocket,
    state: MintState,
    connection_guard: WsConnectionGuard,
) {
    let limits = state.ws_limiter.limits();
    let (publisher, mut subscriber) = mpsc::channel(100);
    let started_at = Instant::now();
    let mut context = WsContext {
        state,
        subscriptions: HashMap::new(),
        topics_in_use: 0,
        budget: RequestBudget::new(&limits, started_at),
        publisher,
        _connection_guard: connection_guard,
    };

    let mut last_activity = started_at;
    let mut keepalive = tokio::time::interval(limits.ping_interval);
    keepalive.set_missed_tick_behavior(MissedTickBehavior::Delay);
    keepalive.tick().await;

    loop {
        tokio::select! {
            Some((sub_id, payload)) = subscriber.recv() => {
                if !context.subscriptions.contains_key(&sub_id) {
                    // It may be possible an incoming message has come from a dropped Subscriptions that has not yet been
                    // unsubscribed from the subscription manager, just ignore it.
                    continue;
                }
                let notification = notification_to_ws_message(NotificationInner {
                    sub_id,
                    payload,
                });
                let message = match serde_json::to_string(&notification) {
                    Ok(message) => message,
                    Err(err) => {
                        tracing::error!("Could not serialize notification: {}", err);
                        continue;
                    }
                };

                if let Err(err)= socket.send(Message::Text(message.into())).await {
                    tracing::error!("Could not send websocket message: {}", err);
                    break;
                }
            }

            _ = keepalive.tick() => {
                if last_activity.elapsed() >= limits.idle_timeout {
                    tracing::debug!("ws-idle: closing after {:?}", last_activity.elapsed());
                    if let Err(err) = socket.send(Message::Close(Some(CloseFrame {
                        code: close_code::POLICY,
                        reason: "idle timeout".into(),
                    }))).await {
                        tracing::debug!("Could not send idle close frame: {err}");
                    }
                    break;
                }

                if let Err(err) = socket.send(Message::Ping(Default::default())).await {
                    tracing::debug!("Could not send keepalive ping: {err}");
                    break;
                }
            }

            from_ws = socket.next() => {
                // The keepalive arm is always ready, so a closed socket no longer
                // falls through to `else`; end the loop here instead of polling a
                // finished stream until the next ping fails.
                let Some(from_ws) = from_ws else {
                    tracing::debug!("ws-close: stream ended");
                    break;
                };

                let message = match from_ws {
                    Ok(message) => message,
                    Err(err) => {
                        tracing::debug!("ws-error: {err}");
                        break;
                    }
                };

                let text = match &message {
                    Message::Text(text) => text.as_str(),
                    Message::Binary(bin) => match std::str::from_utf8(bin) {
                        Ok(text) => text,
                        Err(err) => {
                            tracing::debug!("Could not decode request: {err}");
                            continue;
                        }
                    },
                    // Axum answers pings itself; replying here too would send a
                    // second pong for every ping a client sends.
                    Message::Ping(_) => continue,
                    Message::Pong(_payload) => continue,
                    Message::Close(frame) => {
                        if let Some(CloseFrame { code, reason }) = frame {
                            tracing::info!(
                                "ws-close: code={code:?} reason='{}'",
                                escape_control(reason)
                            );
                        } else {
                            tracing::info!("ws-close: no frame");
                        }

                        if let Err(err) = socket.send(Message::Close(Some(CloseFrame {
                            code: close_code::NORMAL,
                            reason: "bye!".into(),
                        }))).await {
                            tracing::debug!("Could not send close frame: {err}");
                        }
                        break;
                    }
                };

                let now = Instant::now();
                let result = match context.budget.charge(1, now) {
                    Charge::Accepted => {
                        last_activity = now;
                        match deserialize_request(text) {
                            Ok(request) => process(&mut context, request).await,
                            Err(ParseFailure { error, id }) => {
                                tracing::debug!("Could not parse request: {error}");
                                match id {
                                    Some(id) => error_response(id, WsError::InvalidParams),
                                    None => continue,
                                }
                            }
                        }
                    }
                    Charge::Throttled => match recover_request_id(text) {
                        Some(id) => error_response(id, WsError::ServerBusy),
                        None => continue,
                    },
                    Charge::Exhausted => {
                        tracing::debug!("ws-rate: closing after repeated throttling");
                        if let Err(err) = socket.send(Message::Close(Some(CloseFrame {
                            code: close_code::POLICY,
                            reason: "request rate exceeded".into(),
                        }))).await {
                            tracing::debug!("Could not send rate-limit close frame: {err}");
                        }
                        break;
                    }
                };

                match result {
                    Ok(result) => {
                        if let Err(err) = socket
                            .send(Message::Text(result.to_string().into()))
                            .await
                        {
                            tracing::debug!("Could not send request: {}", err);
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
                // Unexpected, we should exit the loop
                tracing::warn!("Unexpected event, closing ws");
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use cdk::mint::{Mint, MintLimits, QuoteId};
    use cdk::nuts::nut02::KeySetVersion;
    use cdk::nuts::nut17::{MAX_CUSTOM_KIND_LEN, MAX_SUBSCRIPTION_ID_LEN};
    use cdk::nuts::{CurrencyUnit, MintInfo};
    use cdk::subscription::{Params, SubId};
    use cdk::ws::WsUnsubscribeRequest;
    use cdk_common::pub_sub::PubsubLimits;
    use cdk_signatory::db_signatory::DbSignatory;
    use cdk_signatory::signatory::{RotateKeyArguments, Signatory};
    use cdk_sqlite::mint::memory;

    use super::*;
    use crate::cache::HttpCache;

    #[test]
    fn oversized_subscription_fields_return_invalid_params() {
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
            let ParseFailure { error, id } =
                deserialize_request(&request.to_string()).expect_err("oversized request");
            assert!(
                error.to_string().contains("exceeds"),
                "unexpected error: {error}"
            );

            let response = error_response(id.expect("request ID"), WsError::InvalidParams)
                .expect("error response");
            assert_eq!(response["error"]["code"], serde_json::json!(-32602));
            assert_eq!(
                response["error"]["message"],
                serde_json::json!("Invalid params")
            );
            assert_eq!(response["jsonrpc"], serde_json::json!("2.0"));
            assert_eq!(response["id"], request["id"]);
        }
    }

    async fn create_test_mint_with_limits(limits: MintLimits) -> Arc<Mint> {
        let localstore = Arc::new(memory::empty().await.expect("in-memory db"));

        let seed = [0u8; 32];
        let mut supported_units = HashMap::new();
        let amounts: Vec<u64> = (0..8).map(|i| 2u64.pow(i)).collect();
        supported_units.insert(CurrencyUnit::Sat, (0u64, amounts));

        let signatory = Arc::new(
            DbSignatory::new(
                localstore.clone(),
                &seed,
                supported_units.clone(),
                HashMap::new(),
            )
            .await
            .expect("signatory"),
        );

        for (unit, (fee, amounts)) in &supported_units {
            signatory
                .rotate_keyset(RotateKeyArguments {
                    unit: unit.clone(),
                    amounts: amounts.clone(),
                    input_fee_ppk: *fee,
                    keyset_id_type: KeySetVersion::Version00,
                    final_expiry: None,
                })
                .await
                .expect("rotate keyset");
        }

        Arc::new(
            Mint::new(
                MintInfo::default(),
                signatory,
                localstore,
                HashMap::new(),
                limits,
            )
            .await
            .expect("mint"),
        )
    }

    async fn create_test_mint() -> Arc<Mint> {
        create_test_mint_with_limits(MintLimits::default()).await
    }

    fn make_params(sub_id: &str) -> Params {
        // A non-empty filter is required so the subscription is registered in
        // the TopicTree and the internal channel stays open.  Without a filter
        // the channel closes immediately and the ActiveSubscription is dropped
        // before the test can observe the active_subscribers count.
        Params {
            kind: cdk::nuts::nut17::Kind::Bolt11MintQuote,
            filters: vec![QuoteId::new().to_string()],
            id: Arc::new(SubId::from(sub_id)),
        }
    }

    fn make_context(mint: Arc<Mint>) -> WsContext {
        make_context_with_limits(mint, WsLimits::default())
    }

    fn make_context_with_limits(mint: Arc<Mint>, limits: WsLimits) -> WsContext {
        let ws_limiter = Arc::new(WsConnectionLimiter::new(limits));
        let connection_guard = ws_limiter.try_acquire(None).expect("connection slot");
        let state = MintState {
            mint,
            cache: Arc::new(HttpCache::default()),
            ws_limiter,
        };
        let (publisher, _receiver) = tokio::sync::mpsc::channel(100);
        WsContext {
            state,
            subscriptions: HashMap::new(),
            topics_in_use: 0,
            budget: RequestBudget::new(&limits, Instant::now()),
            publisher,
            _connection_guard: connection_guard,
        }
    }

    /// Verify that unsubscribing leaks the background task and leaves the
    /// subscription registered in the pub/sub manager.
    ///
    /// This test is expected to FAIL until the fix is applied: after an
    /// explicit unsubscribe the `active_subscribers` count must return to 0,
    /// but the current code only removes the `JoinHandle` from the map without
    /// aborting the task (which owns the `ActiveSubscription`).
    #[tokio::test]
    async fn test_unsubscribe_cleans_up_active_subscription() {
        let mint = create_test_mint().await;
        let pubsub = mint.pubsub_manager();
        let mut context = make_context(mint);

        // Subscribe
        subscribe::handle(&mut context, make_params("sub-1"))
            .await
            .expect("subscribe");

        // Give the spawned task a moment to register
        tokio::task::yield_now().await;

        assert_eq!(
            pubsub.active_subscribers(),
            1,
            "should have 1 active subscriber after subscribe"
        );

        // Unsubscribe
        unsubscribe::handle(
            &mut context,
            WsUnsubscribeRequest {
                sub_id: Arc::new(SubId::from("sub-1")),
            },
        )
        .await
        .expect("unsubscribe");

        // The task must be aborted and the ActiveSubscription dropped so the
        // pub/sub index is cleaned up.  Without the fix this will be 1.
        tokio::task::yield_now().await;
        assert_eq!(
            pubsub.active_subscribers(),
            0,
            "active_subscribers should be 0 after explicit unsubscribe"
        );
    }

    /// Verify that dropping the `WsContext` (i.e. client disconnect) leaks
    /// background tasks and leaves subscriptions registered in the pub/sub
    /// manager.
    ///
    /// This test is expected to FAIL until the fix is applied: when the
    /// context is dropped all spawned tasks must be aborted so the
    /// `ActiveSubscription` destructor cleans up the pub/sub indexes.
    #[tokio::test]
    async fn test_context_drop_cleans_up_active_subscriptions() {
        let mint = create_test_mint().await;
        let pubsub = mint.pubsub_manager();
        let mut context = make_context(mint);

        // Subscribe twice with different IDs
        subscribe::handle(&mut context, make_params("sub-A"))
            .await
            .expect("subscribe A");
        subscribe::handle(&mut context, make_params("sub-B"))
            .await
            .expect("subscribe B");

        tokio::task::yield_now().await;
        assert_eq!(
            pubsub.active_subscribers(),
            2,
            "should have 2 active subscribers"
        );

        // Simulate client disconnect by dropping the context
        drop(context);

        // All tasks must be aborted and both ActiveSubscriptions dropped.
        // Without the fix this will remain 2.
        tokio::task::yield_now().await;
        assert_eq!(
            pubsub.active_subscribers(),
            0,
            "active_subscribers should be 0 after context drop (disconnect)"
        );
    }

    #[tokio::test]
    async fn test_per_connection_subscription_count_limit() {
        let cap = WsLimits::default().max_subscriptions_per_connection;
        let mint = create_test_mint().await;
        let pubsub = mint.pubsub_manager();
        let mut context = make_context(mint);

        for i in 0..cap {
            subscribe::handle(&mut context, make_params(&format!("sub-cap-{i}")))
                .await
                .expect("subscribe before cap should succeed");
        }

        tokio::task::yield_now().await;
        assert_eq!(
            pubsub.active_subscribers(),
            cap,
            "should have subscribers up to the per-connection cap"
        );

        let over_cap =
            subscribe::handle(&mut context, make_params(&format!("sub-cap-{cap}"))).await;

        assert!(
            matches!(over_cap, Err(WsError::ServerBusy)),
            "subscription over the per-connection cap should be rejected as busy"
        );
        assert_eq!(
            pubsub.active_subscribers(),
            cap,
            "rejected subscription should not allocate a pub/sub subscriber"
        );
    }

    #[tokio::test]
    async fn test_subscription_filter_count_not_tied_to_max_inputs() {
        let mint = create_test_mint_with_limits(MintLimits {
            max_inputs: 2,
            max_outputs: 2,
            ..MintLimits::default()
        })
        .await;
        let mut context = make_context(mint);

        let params = Params {
            kind: cdk::nuts::nut17::Kind::Bolt11MintQuote,
            filters: (0..5).map(|_| QuoteId::new().to_string()).collect(),
            id: Arc::new(SubId::from("sub-many-filters")),
        };

        let result = subscribe::handle(&mut context, params).await;
        assert!(
            result.is_ok(),
            "subscription filter count must not be capped by mint max_inputs; got {:?}",
            result.as_ref().err()
        );
    }

    fn make_params_with_filters(sub_id: &str, filters: usize) -> Params {
        Params {
            kind: cdk::nuts::nut17::Kind::Bolt11MintQuote,
            filters: (0..filters).map(|_| QuoteId::new().to_string()).collect(),
            id: Arc::new(SubId::from(sub_id)),
        }
    }

    #[tokio::test]
    async fn test_per_connection_topic_budget() {
        let mint = create_test_mint().await;
        let pubsub = mint.pubsub_manager();
        let limits = WsLimits {
            max_topics_per_connection: 10,
            ..WsLimits::default()
        };
        let mut context = make_context_with_limits(mint, limits);

        subscribe::handle(&mut context, make_params_with_filters("sub-a", 6))
            .await
            .expect("first subscription fits in the budget");
        assert_eq!(context.topics_in_use, 6);

        let over_budget =
            subscribe::handle(&mut context, make_params_with_filters("sub-b", 5)).await;

        assert!(
            matches!(over_budget, Err(WsError::ServerBusy)),
            "subscription over the per-connection topic budget should be rejected as busy"
        );
        assert_eq!(
            context.topics_in_use, 6,
            "rejection must not consume budget"
        );

        tokio::task::yield_now().await;
        assert_eq!(
            pubsub.registered_topics(),
            6,
            "rejected subscription must not register topics in the mint-wide index"
        );
    }

    #[tokio::test]
    async fn a_subscription_over_the_request_budget_is_refused_as_busy() {
        let mint = create_test_mint().await;
        let pubsub = mint.pubsub_manager();
        let limits = WsLimits {
            max_request_units_per_second: 1,
            max_request_burst_units: 5,
            max_throttled_requests: 0,
            ..WsLimits::default()
        };
        let mut context = make_context_with_limits(mint, limits);

        let over_budget =
            subscribe::handle(&mut context, make_params_with_filters("sub-a", 8)).await;

        assert!(
            matches!(over_budget, Err(WsError::ServerBusy)),
            "subscription over the connection's request budget should be rejected as busy"
        );
        assert_eq!(
            context.topics_in_use, 0,
            "rejection must not consume budget"
        );
        assert!(context.subscriptions.is_empty());

        tokio::task::yield_now().await;
        assert_eq!(
            pubsub.registered_topics(),
            0,
            "a throttled subscription must leave no state behind"
        );
        assert_eq!(pubsub.active_subscribers(), 0);
    }

    /// A subscription costs one unit per filter, so churning wide subscriptions
    /// drains the budget far faster than the per-connection topic cap alone
    /// would suggest.
    #[tokio::test]
    async fn subscribe_churn_is_charged_per_filter() {
        let mint = create_test_mint().await;
        let limits = WsLimits {
            max_request_units_per_second: 1,
            max_request_burst_units: 12,
            max_throttled_requests: 0,
            ..WsLimits::default()
        };
        let mut context = make_context_with_limits(mint, limits);

        for round in 0..2 {
            let sub_id = format!("sub-{round}");
            subscribe::handle(&mut context, make_params_with_filters(&sub_id, 5))
                .await
                .expect("subscription fits in the request budget");

            unsubscribe::handle(
                &mut context,
                WsUnsubscribeRequest {
                    sub_id: Arc::new(SubId::from(sub_id.as_str())),
                },
            )
            .await
            .expect("unsubscribe");
        }

        let exhausted = subscribe::handle(&mut context, make_params_with_filters("sub-2", 5)).await;

        assert!(
            matches!(exhausted, Err(WsError::ServerBusy)),
            "a third round of churn must exhaust the request budget"
        );
    }

    #[tokio::test]
    async fn a_disabled_rate_never_refuses_a_subscription() {
        let mint = create_test_mint().await;
        let limits = WsLimits {
            max_request_units_per_second: 0,
            max_request_burst_units: 1,
            ..WsLimits::default()
        };
        let mut context = make_context_with_limits(mint, limits);

        for round in 0..4 {
            subscribe::handle(
                &mut context,
                make_params_with_filters(&format!("sub-{round}"), 5),
            )
            .await
            .expect("a disabled throttle admits every request");
        }
    }

    #[tokio::test]
    async fn test_topic_budget_released_on_unsubscribe() {
        let mint = create_test_mint().await;
        let limits = WsLimits {
            max_topics_per_connection: 10,
            ..WsLimits::default()
        };
        let mut context = make_context_with_limits(mint, limits);

        subscribe::handle(&mut context, make_params_with_filters("sub-a", 8))
            .await
            .expect("first subscription fits in the budget");
        assert!(
            subscribe::handle(&mut context, make_params_with_filters("sub-b", 8))
                .await
                .is_err()
        );

        unsubscribe::handle(
            &mut context,
            WsUnsubscribeRequest {
                sub_id: Arc::new(SubId::from("sub-a")),
            },
        )
        .await
        .expect("unsubscribe");

        assert_eq!(context.topics_in_use, 0);
        subscribe::handle(&mut context, make_params_with_filters("sub-b", 8))
            .await
            .expect("budget is available again after unsubscribe");
    }

    #[tokio::test]
    async fn test_mint_wide_topic_budget_maps_to_server_busy() {
        let mint = create_test_mint_with_limits(MintLimits {
            pubsub: PubsubLimits {
                max_topics: 1,
                ..PubsubLimits::default()
            },
            ..MintLimits::default()
        })
        .await;
        let mut first = make_context(mint.clone());
        let mut second = make_context(mint);

        subscribe::handle(&mut first, make_params("sub-a"))
            .await
            .expect("first subscription takes the whole mint-wide budget");

        let over_budget = subscribe::handle(&mut second, make_params("sub-b")).await;
        assert!(
            matches!(over_budget, Err(WsError::ServerBusy)),
            "a second connection must be refused once the mint-wide budget is gone"
        );

        let body = cdk::ws::WsErrorBody::from(WsError::ServerBusy);
        assert_eq!(body.code, -32000);
    }

    /// Drives a real handshake over TCP: the `WebSocketUpgrade` extractor needs
    /// hyper's upgrade extension, which an in-memory request does not carry.
    async fn handshake_status(limits: WsLimits, connections: usize) -> Vec<u16> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::{TcpListener, TcpStream};

        let ws_limiter = Arc::new(WsConnectionLimiter::new(limits));
        let state = MintState {
            mint: create_test_mint().await,
            cache: Arc::new(HttpCache::default()),
            ws_limiter,
        };
        let router = axum::Router::new()
            .route(
                "/v1/ws",
                axum::routing::get(crate::router_handlers::ws_handler),
            )
            .with_state(state);

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let server = tokio::spawn(async move {
            let service = router.into_make_service_with_connect_info::<std::net::SocketAddr>();
            if let Err(err) = axum::serve(listener, service).await {
                tracing::debug!("test server stopped: {err}");
            }
        });

        let mut statuses = Vec::new();
        // Held open so each handshake sees the slots the previous ones took.
        let mut sockets = Vec::new();

        for _ in 0..connections {
            let mut socket = TcpStream::connect(addr).await.expect("connect");
            socket
                .write_all(
                    format!(
                        "GET /v1/ws HTTP/1.1\r\nHost: {addr}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await
                .expect("write handshake");

            let mut buf = [0u8; 64];
            let read = socket.read(&mut buf).await.expect("read status line");
            let status = String::from_utf8_lossy(&buf[..read])
                .split_whitespace()
                .nth(1)
                .and_then(|code| code.parse().ok())
                .expect("status code");

            statuses.push(status);
            sockets.push(socket);
        }

        server.abort();
        statuses
    }

    #[tokio::test]
    async fn handshake_is_refused_once_the_mint_is_at_capacity() {
        let statuses = handshake_status(
            WsLimits {
                max_connections: 1,
                max_connections_per_ip: 0,
                ..WsLimits::default()
            },
            2,
        )
        .await;

        assert_eq!(statuses, vec![101, 503]);
    }

    /// Also covers the connect-info plumbing: without it the handler sees no peer
    /// address and this limit silently does nothing.
    #[tokio::test]
    async fn handshake_is_refused_once_the_peer_address_is_at_capacity() {
        let statuses = handshake_status(
            WsLimits {
                max_connections: 8,
                max_connections_per_ip: 1,
                ..WsLimits::default()
            },
            2,
        )
        .await;

        assert_eq!(statuses, vec![101, 429]);
    }
}

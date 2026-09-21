use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use cdk::mint::QuoteId;
use cdk::nuts::nut17::ws::{WsErrorResponse, JSON_RPC_VERSION};
use cdk::nuts::nut17::NotificationPayload;
use cdk::subscription::SubId;
use cdk::ws::{
    notification_to_ws_message, NotificationInner, WsErrorBody, WsMessageOrResponse,
    WsMethodRequest, WsRequest,
};
use cdk_common::terminal::escape_control;
use futures::StreamExt;
use serde_json::error::Category;
use tokio::sync::mpsc;

use crate::MintState;

mod error;
mod subscribe;
mod unsubscribe;

pub(crate) const MAX_SUBSCRIPTIONS_PER_CONNECTION: usize = 100;
pub(crate) const MAX_FILTERS_PER_SUBSCRIPTION: usize = 1000;

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

/// Why a frame is not processed, and whether the client hears about it.
#[derive(Debug)]
enum Rejection {
    /// A notification, which JSON-RPC 2.0 section 4.1 forbids answering.
    Ignored,
    /// An error to send back, with the request id when one could be recovered.
    Answered(WsError, Option<usize>),
}

/// Parse a request, and on failure say whether the client is answered at all,
/// with which JSON-RPC error, and which request id can still be echoed back.
///
/// The happy path deserializes straight from the text. Only a rejected frame
/// pays for the `serde_json::Value` tree, which is several times the size of the
/// message and is needed solely to classify the failure and recover the id. Any
/// category but [`Category::Data`] means the bytes were not JSON at all, so
/// there is no document to read an id out of.
fn deserialize_request(text: &str) -> Result<WsRequest, Rejection> {
    let err = match serde_json::from_str::<WsRequest>(text) {
        Ok(request) => return Ok(request),
        Err(err) => err,
    };

    if err.classify() != Category::Data {
        return Err(Rejection::Answered(WsError::ParseError, None));
    }

    let value = match serde_json::from_str::<serde_json::Value>(text) {
        Ok(value) => value,
        Err(reparse_err) => {
            tracing::debug!("Could not re-read a rejected ws request: {reparse_err}");
            return Err(Rejection::Answered(WsError::ParseError, None));
        }
    };

    Err(classify_request(&value, &err))
}

/// Decide what a frame that is valid JSON but not a valid request earns.
///
/// The envelope is judged structurally rather than from serde's wording, and the
/// order is load-bearing: only a well-formed 2.0 request object can be a
/// notification, so a bad envelope is still answered with -32600 instead of
/// being dropped. An `id` that is present but cannot be a `usize` is a request
/// this server cannot answer by id, so it earns -32600 as well.
///
/// The one thing the document cannot answer is whether a well-formed method name
/// is one the enum knows, since the variants live in `cashu`; serde reports that
/// as an `unknown variant` error, and `unknown_method_is_reported_as_unknown_variant`
/// pins that wording so a serde change cannot quietly turn -32601 back into
/// -32602.
fn classify_request(value: &serde_json::Value, err: &serde_json::Error) -> Rejection {
    let invalid_request = Rejection::Answered(WsError::InvalidRequest, None);

    let Some(object) = value.as_object() else {
        return invalid_request;
    };

    if object.get("jsonrpc").and_then(serde_json::Value::as_str) != Some(JSON_RPC_VERSION) {
        return invalid_request;
    }

    if object
        .get("method")
        .and_then(serde_json::Value::as_str)
        .is_none()
    {
        return invalid_request;
    }

    let Some(id) = object.get("id") else {
        return Rejection::Ignored;
    };

    let Some(request_id) = id.as_u64().and_then(|id| usize::try_from(id).ok()) else {
        return invalid_request;
    };

    if err.to_string().starts_with("unknown variant") {
        Rejection::Answered(WsError::MethodNotFound, Some(request_id))
    } else {
        Rejection::Answered(WsError::InvalidParams, Some(request_id))
    }
}

fn error_response(
    request_id: Option<usize>,
    error: WsError,
) -> Result<serde_json::Value, serde_json::Error> {
    let response: WsMessageOrResponse =
        WsErrorResponse::new(request_id, WsErrorBody::from(error)).into();
    serde_json::to_value(response)
}

pub use error::WsError;

pub struct WsContext {
    state: MintState,
    subscriptions: HashMap<Arc<SubId>, tokio::task::JoinHandle<()>>,
    publisher: mpsc::Sender<(Arc<SubId>, NotificationPayload<QuoteId>)>,
}

impl Drop for WsContext {
    fn drop(&mut self) {
        for (_, handle) in self.subscriptions.drain() {
            handle.abort();
        }
    }
}

/// Main function for websocket connections
///
/// This function will handle all incoming websocket connections and keep them in their own loop.
///
/// For simplicity sake this function will spawn tasks for each subscription and
/// keep them in a hashmap, and will have a single subscriber for all of them.
pub async fn main_websocket(mut socket: WebSocket, state: MintState) {
    let (publisher, mut subscriber) = mpsc::channel(100);
    let mut context = WsContext {
        state,
        subscriptions: HashMap::new(),
        publisher,
    };

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

            from_ws = socket.next() => {
                let Some(from_ws) = from_ws else {
                    break;
                };
                let text = match from_ws {
                    Ok(Message::Text(text)) => text.to_string(),
                    Ok(Message::Binary(bin)) => String::from_utf8_lossy(&bin).to_string(),
                    Ok(Message::Ping(payload)) => {
                        // Reply with Pong with same payload
                        if let Err(e) = socket.send(Message::Pong(payload)).await {
                            tracing::error!("failed to send pong: {e}");
                            break;
                        }
                        continue;
                    },
                    Ok(Message::Pong(_payload)) => {
                        tracing::error!("Unexpected pong");
                        continue;
                    },
                    Ok(Message::Close(frame)) => {
                        if let Some(CloseFrame { code, reason }) = frame {
                            tracing::info!(
                                "ws-close: code={code:?} reason='{}'",
                                escape_control(&reason)
                            );
                        } else {
                            tracing::info!("ws-close: no frame");
                        }

                        let _ = socket.send(Message::Close(Some(CloseFrame {
                            code: axum::extract::ws::close_code::NORMAL,
                            reason: "bye!".into(),
                        }))).await;
                        break;
                    }
                    Err(err) => {
                        tracing::error!("ws-error: {err}");
                        break;
                    }
                };


                let result = match deserialize_request(&text) {
                    Ok(request) => process(&mut context, request).await,
                    Err(Rejection::Ignored) => continue,
                    Err(Rejection::Answered(err, request_id)) => {
                        tracing::error!("Rejected ws request: {err:?}");
                        error_response(request_id, err)
                    }
                };

                match result {
                    Ok(result) => {
                        if let Err(err) = socket
                            .send(Message::Text(result.to_string().into()))
                            .await
                        {
                            tracing::error!("Could not send request: {}", err);
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

    use cdk::mint::{Mint, QuoteId};
    use cdk::nuts::nut02::KeySetVersion;
    use cdk::nuts::nut17::{MAX_CUSTOM_KIND_LEN, MAX_FILTER_LEN, MAX_SUBSCRIPTION_ID_LEN};
    use cdk::nuts::{CurrencyUnit, MintInfo};
    use cdk::subscription::{Params, SubId};
    use cdk::ws::WsUnsubscribeRequest;
    use cdk_signatory::db_signatory::DbSignatory;
    use cdk_signatory::signatory::{RotateKeyArguments, Signatory};
    use cdk_sqlite::mint::memory;

    use super::*;
    use crate::cache::HttpCache;

    fn rejection_of(text: &str) -> serde_json::Value {
        match deserialize_request(text).expect_err("a rejected request") {
            Rejection::Answered(err, request_id) => {
                error_response(request_id, err).expect("error response")
            }
            Rejection::Ignored => panic!("{text} should have been answered"),
        }
    }

    fn is_ignored(text: &str) -> bool {
        matches!(
            deserialize_request(text).expect_err("a rejected request"),
            Rejection::Ignored
        )
    }

    /// JSON-RPC 2.0 wants the `id` member present and null here, not absent.
    #[test]
    fn malformed_json_is_a_parse_error() {
        for text in ["{not json", "", "[1, 2", "{\"jsonrpc\": }"] {
            let response = rejection_of(text);
            assert_eq!(
                response["error"]["code"],
                serde_json::json!(-32700),
                "{text:?} should be a parse error"
            );
            assert_eq!(
                response["error"]["message"],
                serde_json::json!("Parse error")
            );
            assert_eq!(response["id"], serde_json::Value::Null);
            assert!(response.get("id").is_some(), "id member must be present");
        }
    }

    #[test]
    fn unknown_method_is_method_not_found() {
        let response = rejection_of(
            &serde_json::json!({
                "jsonrpc": "2.0",
                "method": "resubscribe",
                "params": {},
                "id": 4,
            })
            .to_string(),
        );
        assert_eq!(response["error"]["code"], serde_json::json!(-32601));
        assert_eq!(
            response["error"]["message"],
            serde_json::json!("Method not found")
        );
        assert_eq!(response["id"], serde_json::json!(4));
    }

    /// `classify_request` leans on this wording to tell an unknown method from
    /// bad params, since only serde knows the enum's variants.
    #[test]
    fn unknown_method_is_reported_as_unknown_variant() {
        let err = serde_json::from_str::<WsRequest>(
            r#"{"jsonrpc":"2.0","method":"resubscribe","params":{},"id":1}"#,
        )
        .expect_err("unknown method");
        assert!(
            err.to_string().starts_with("unknown variant"),
            "serde changed its unknown-variant wording: {err}"
        );
    }

    #[test]
    fn a_malformed_envelope_is_an_invalid_request() {
        for request in [
            serde_json::json!({"jsonrpc": "2.0", "params": {}, "id": 5}),
            serde_json::json!({"jsonrpc": "2.0", "method": 7, "params": {}, "id": 5}),
            serde_json::json!({"jsonrpc": "1.0", "method": "subscribe", "params": {}, "id": 5}),
            serde_json::json!({"method": "subscribe", "params": {}, "id": 5}),
            serde_json::json!([1, 2, 3]),
        ] {
            let response = rejection_of(&request.to_string());
            assert_eq!(
                response["error"]["code"],
                serde_json::json!(-32600),
                "{request} should be an invalid request"
            );
            assert_eq!(
                response["error"]["message"],
                serde_json::json!("Invalid Request")
            );
        }
    }

    /// JSON-RPC 2.0 section 4.1: the server must not reply to a notification,
    /// whatever else is wrong with it.
    #[test]
    fn a_notification_gets_no_reply() {
        for request in [
            serde_json::json!({
                "jsonrpc": "2.0",
                "method": "subscribe",
                "params": {
                    "kind": "bolt11_mint_quote",
                    "filters": ["quote-id"],
                    "subId": "sub-1",
                },
            }),
            serde_json::json!({"jsonrpc": "2.0", "method": "resubscribe", "params": {}}),
            serde_json::json!({"jsonrpc": "2.0", "method": "subscribe", "params": {}}),
        ] {
            assert!(
                is_ignored(&request.to_string()),
                "{request} should get no reply"
            );
        }
    }

    /// A frame that is not a well-formed request object is not a notification.
    #[test]
    fn an_id_less_malformed_envelope_is_still_answered() {
        for request in [
            serde_json::json!({"method": "subscribe", "params": {}}),
            serde_json::json!({"jsonrpc": "2.0", "params": {}}),
            serde_json::json!({"jsonrpc": "1.0", "method": "subscribe", "params": {}}),
        ] {
            let response = rejection_of(&request.to_string());
            assert_eq!(
                response["error"]["code"],
                serde_json::json!(-32600),
                "{request} should be an invalid request"
            );
        }
    }

    /// An id this server cannot echo makes the frame an invalid request, not a
    /// params failure.
    #[test]
    fn an_unusable_id_is_an_invalid_request() {
        for id in [
            serde_json::json!("abc"),
            serde_json::json!(-1),
            serde_json::json!(1.5),
            serde_json::Value::Null,
        ] {
            let request = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "subscribe",
                "params": {},
                "id": id,
            });
            let response = rejection_of(&request.to_string());
            assert_eq!(
                response["error"]["code"],
                serde_json::json!(-32600),
                "{request} should be an invalid request"
            );
            assert_eq!(response["id"], serde_json::Value::Null);
            assert!(response.get("id").is_some(), "id member must be present");
        }
    }

    /// The envelope and method checks must not reject good traffic.
    #[test]
    fn a_valid_subscribe_still_parses() {
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "subscribe",
            "params": {
                "kind": "bolt11_mint_quote",
                "filters": ["quote-id"],
                "subId": "sub-1",
            },
            "id": 6,
        });
        let parsed = deserialize_request(&request.to_string()).expect("a valid subscribe");
        assert_eq!(parsed.id, 6);
        assert!(matches!(parsed.method, WsMethodRequest::Subscribe(_)));
    }

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
            let response = rejection_of(&request.to_string());
            assert_eq!(response["error"]["code"], serde_json::json!(-32602));
            assert_eq!(
                response["error"]["message"],
                serde_json::json!("Invalid params")
            );
            assert_eq!(response["jsonrpc"], serde_json::json!("2.0"));
            assert_eq!(response["id"], request["id"]);
        }
    }

    async fn create_test_mint_with_limits(max_inputs: usize, max_outputs: usize) -> Arc<Mint> {
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
                max_inputs,
                max_outputs,
            )
            .await
            .expect("mint"),
        )
    }

    async fn create_test_mint() -> Arc<Mint> {
        create_test_mint_with_limits(1000, 1000).await
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
        let state = MintState {
            mint,
            cache: Arc::new(HttpCache::default()),
        };
        let (publisher, _receiver) = tokio::sync::mpsc::channel(100);
        WsContext {
            state,
            subscriptions: HashMap::new(),
            publisher,
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
        let mint = create_test_mint().await;
        let pubsub = mint.pubsub_manager();
        let mut context = make_context(mint);

        for i in 0..MAX_SUBSCRIPTIONS_PER_CONNECTION {
            subscribe::handle(&mut context, make_params(&format!("sub-cap-{i}")))
                .await
                .expect("subscribe before cap should succeed");
        }

        tokio::task::yield_now().await;
        assert_eq!(
            pubsub.active_subscribers(),
            MAX_SUBSCRIPTIONS_PER_CONNECTION,
            "should have subscribers up to the per-connection cap"
        );

        let over_cap = subscribe::handle(
            &mut context,
            make_params(&format!("sub-cap-{MAX_SUBSCRIPTIONS_PER_CONNECTION}")),
        )
        .await;

        assert!(
            over_cap.is_err(),
            "subscription over the per-connection cap should be rejected"
        );
        assert_eq!(
            pubsub.active_subscribers(),
            MAX_SUBSCRIPTIONS_PER_CONNECTION,
            "rejected subscription should not allocate a pub/sub subscriber"
        );
    }

    #[tokio::test]
    async fn test_subscription_filter_count_not_tied_to_max_inputs() {
        let mint = create_test_mint_with_limits(2, 2).await;
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

    #[tokio::test]
    async fn test_bad_filters_are_invalid_params_not_internal_error() {
        let mint = create_test_mint().await;
        let mut context = make_context(mint);

        for (name, filter) in [
            ("unparsable", "not-a-quote-id".to_string()),
            ("oversized", "a".repeat(MAX_FILTER_LEN + 1)),
        ] {
            let params = Params {
                kind: cdk::nuts::nut17::Kind::Bolt11MintQuote,
                filters: vec![filter],
                id: Arc::new(SubId::from(name)),
            };

            let err = subscribe::handle(&mut context, params)
                .await
                .expect_err("a client-supplied bad filter should be rejected");
            let body: WsErrorBody = err.into();
            assert_eq!(body.code, -32602, "{name} filter should be invalid params");
        }
    }
}

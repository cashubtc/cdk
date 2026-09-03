use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use cdk::error::ErrorCode;
use cdk::mint::QuoteId;
use cdk::nuts::nut17::NotificationPayload;
use cdk::subscription::SubId;
use cdk::ws::{
    notification_to_ws_message, NotificationInner, WsErrorBody, WsMessageOrResponse,
    WsMethodRequest, WsRequest,
};
use cdk_common::terminal::escape_control;
use futures::StreamExt;
use tokio::sync::mpsc;

use crate::MintState;

mod authenticate;
mod error;
mod subscribe;
mod unsubscribe;

pub(crate) const MAX_SUBSCRIPTIONS_PER_CONNECTION: usize = 100;
pub(crate) const MAX_FILTERS_PER_SUBSCRIPTION: usize = 1000;

/// How long a connection that requires blind auth may stay open without
/// authenticating before the mint closes it (NUT-22 SHOULD).
const AUTH_TIMEOUT: Duration = Duration::from_secs(30);

/// How many rejected `authenticate` commands a connection may make before the
/// mint closes it.
///
/// Verifying a blind auth token costs an elliptic curve verification, and until
/// the connection authenticates nobody has paid for it. Without a cap a single
/// socket could burn that work in a loop for the whole [`AUTH_TIMEOUT`] window.
/// The allowance is above one so a client that races a stale token can retry.
const MAX_FAILED_AUTH_ATTEMPTS: usize = 3;

fn blind_auth_required() -> WsError {
    WsError::ServerError(
        ErrorCode::BlindAuthRequired.to_code() as i32,
        "Endpoint requires blind auth".to_string(),
    )
}

async fn process(
    context: &mut WsContext,
    body: WsRequest,
) -> Result<serde_json::Value, serde_json::Error> {
    let response = match body.method {
        WsMethodRequest::Authenticate(req) => authenticate::handle(context, req).await,
        WsMethodRequest::Subscribe(sub) => {
            if context.authenticated {
                subscribe::handle(context, sub).await
            } else {
                Err(blind_auth_required())
            }
        }
        WsMethodRequest::Unsubscribe(unsub) => {
            if context.authenticated {
                unsubscribe::handle(context, unsub).await
            } else {
                Err(blind_auth_required())
            }
        }
    }
    .map_err(WsErrorBody::from);

    let response: WsMessageOrResponse = (body.id, response).into();

    serde_json::to_value(response)
}

fn deserialize_request(text: &str) -> Result<WsRequest, (serde_json::Error, Option<usize>)> {
    let value = serde_json::from_str::<serde_json::Value>(text).map_err(|err| (err, None))?;
    let request_id = value
        .get("id")
        .and_then(serde_json::Value::as_u64)
        .and_then(|id| usize::try_from(id).ok());

    serde_json::from_value(value).map_err(|err| (err, request_id))
}

fn error_response(
    request_id: usize,
    error: WsError,
) -> Result<serde_json::Value, serde_json::Error> {
    let response: WsMessageOrResponse = (request_id, Err(WsErrorBody::from(error))).into();
    serde_json::to_value(response)
}

pub use error::WsError;

/// Send a policy close frame. Best effort: the connection is dropped either way.
async fn close(socket: &mut WebSocket, reason: &'static str) {
    if let Err(err) = socket
        .send(Message::Close(Some(CloseFrame {
            code: axum::extract::ws::close_code::POLICY,
            reason: reason.into(),
        })))
        .await
    {
        tracing::debug!("Could not send close frame: {}", err);
    }
}

pub struct WsContext {
    state: MintState,
    subscriptions: HashMap<Arc<SubId>, tokio::task::JoinHandle<()>>,
    publisher: mpsc::Sender<(Arc<SubId>, NotificationPayload<QuoteId>)>,
    /// Whether the connection may subscribe. Set at upgrade time for open
    /// endpoints and header-authenticated connections, or by a successful
    /// in-band `authenticate` command.
    authenticated: bool,
    /// Rejected in-band `authenticate` commands so far, bounded by
    /// [`MAX_FAILED_AUTH_ATTEMPTS`].
    failed_auth_attempts: usize,
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
pub async fn main_websocket(mut socket: WebSocket, state: MintState, authenticated: bool) {
    let (publisher, mut subscriber) = mpsc::channel(100);
    let mut context = WsContext {
        state,
        subscriptions: HashMap::new(),
        publisher,
        authenticated,
        failed_auth_attempts: 0,
    };

    let auth_timeout = tokio::time::sleep(AUTH_TIMEOUT);
    tokio::pin!(auth_timeout);

    loop {
        tokio::select! {
            // Close connections that never authenticate. The guard disables
            // this branch once the connection is authenticated, so open and
            // authenticated connections are never closed by it.
            () = &mut auth_timeout, if !context.authenticated => {
                tracing::info!("Closing websocket: no authentication within timeout");
                close(&mut socket, "authentication required").await;
                break;
            }
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

            Some(from_ws) = socket.next() => {
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
                    Err((err, request_id)) => {
                        tracing::error!("Could not parse request: {}", err);
                        match request_id {
                            Some(request_id) => {
                                error_response(request_id, WsError::InvalidParams)
                            }
                            None => continue,
                        }
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

                        if context.failed_auth_attempts >= MAX_FAILED_AUTH_ATTEMPTS {
                            tracing::info!("Closing websocket: too many failed authentications");
                            close(&mut socket, "authentication failed").await;
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
    use cdk::nuts::nut17::{MAX_CUSTOM_KIND_LEN, MAX_SUBSCRIPTION_ID_LEN};
    use cdk::nuts::{CurrencyUnit, MintInfo};
    use cdk::subscription::{Params, SubId};
    use cdk::ws::WsUnsubscribeRequest;
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
            let (err, request_id) =
                deserialize_request(&request.to_string()).expect_err("oversized request");
            assert!(
                err.to_string().contains("exceeds"),
                "unexpected error: {err}"
            );

            let response = error_response(request_id.expect("request ID"), WsError::InvalidParams)
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
        make_context_with_auth(mint, true)
    }

    fn make_context_with_auth(mint: Arc<Mint>, authenticated: bool) -> WsContext {
        let state = MintState {
            mint,
            cache: Arc::new(HttpCache::default()),
        };
        let (publisher, _receiver) = tokio::sync::mpsc::channel(100);
        WsContext {
            state,
            subscriptions: HashMap::new(),
            publisher,
            authenticated,
            failed_auth_attempts: 0,
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

    fn subscribe_request(sub_id: &str) -> WsRequest {
        WsRequest {
            jsonrpc: "2.0".to_string(),
            method: WsMethodRequest::Subscribe(make_params(sub_id)),
            id: 1,
        }
    }

    #[tokio::test]
    async fn unauthenticated_subscribe_is_rejected_with_31001() {
        let mint = create_test_mint().await;
        let mut context = make_context_with_auth(mint, false);

        let response = process(&mut context, subscribe_request("sub-unauth"))
            .await
            .expect("process serializes");

        assert_eq!(response["error"]["code"], 31001);
        assert!(
            context.subscriptions.is_empty(),
            "a rejected subscribe must not register a subscription"
        );
    }

    #[tokio::test]
    async fn unauthenticated_unsubscribe_is_rejected_with_31001() {
        let mint = create_test_mint().await;
        let mut context = make_context_with_auth(mint, false);

        let request = WsRequest {
            jsonrpc: "2.0".to_string(),
            method: WsMethodRequest::Unsubscribe(WsUnsubscribeRequest {
                sub_id: Arc::new(SubId::from("sub-unauth")),
            }),
            id: 2,
        };

        let response = process(&mut context, request)
            .await
            .expect("process serializes");

        assert_eq!(response["error"]["code"], 31001);
    }

    #[tokio::test]
    async fn authenticate_with_garbage_token_returns_31002() {
        let mint = create_test_mint().await;
        let mut context = make_context_with_auth(mint, false);

        let err = authenticate::handle(
            &mut context,
            cdk::ws::WsAuthenticateRequest {
                token: "not-a-valid-bat".to_string(),
            },
        )
        .await
        .expect_err("garbage token must fail");

        let body = cdk::ws::WsErrorBody::from(err);
        assert_eq!(body.code, 31002);
        assert!(
            !context.authenticated,
            "a failed authenticate must not authenticate the connection"
        );
    }

    #[tokio::test]
    async fn authenticated_subscribe_is_accepted() {
        let mint = create_test_mint().await;
        let mut context = make_context_with_auth(mint, true);

        let response = process(&mut context, subscribe_request("sub-authed"))
            .await
            .expect("process serializes");

        assert_eq!(response["result"]["status"], "OK");
    }

    #[tokio::test]
    async fn failed_authenticate_attempts_are_counted_up_to_the_cap() {
        let mint = create_test_mint().await;
        let mut context = make_context_with_auth(mint, false);

        for expected in 1..=MAX_FAILED_AUTH_ATTEMPTS {
            let response = process(
                &mut context,
                WsRequest {
                    jsonrpc: "2.0".to_string(),
                    method: WsMethodRequest::Authenticate(cdk::ws::WsAuthenticateRequest {
                        token: "not-a-valid-bat".to_string(),
                    }),
                    id: expected,
                },
            )
            .await
            .expect("process serializes");

            assert_eq!(response["error"]["code"], 31002);
            assert_eq!(context.failed_auth_attempts, expected);
        }

        assert!(
            context.failed_auth_attempts >= MAX_FAILED_AUTH_ATTEMPTS,
            "the connection must be closable once the cap is reached"
        );
    }

    #[tokio::test]
    async fn repeat_authenticate_on_an_authenticated_connection_is_not_a_failure() {
        let mint = create_test_mint().await;
        let mut context = make_context_with_auth(mint, true);

        authenticate::handle(
            &mut context,
            cdk::ws::WsAuthenticateRequest {
                token: "not-a-valid-bat".to_string(),
            },
        )
        .await
        .expect("no-op");

        assert_eq!(
            context.failed_auth_attempts, 0,
            "an already-authenticated connection must not accrue failures"
        );
    }

    #[tokio::test]
    async fn authenticate_is_idempotent_when_already_authenticated() {
        let mint = create_test_mint().await;
        let mut context = make_context_with_auth(mint, true);

        // Even an invalid token succeeds: an already-authenticated connection
        // must short-circuit before any parse/verify/burn, so a repeat
        // authenticate never spends a second BAT.
        let result = authenticate::handle(
            &mut context,
            cdk::ws::WsAuthenticateRequest {
                token: "not-a-valid-bat".to_string(),
            },
        )
        .await
        .expect("repeat authenticate on an authenticated connection is a no-op");

        assert!(matches!(result, cdk::ws::WsResponseResult::Authenticate(_)));
        assert!(context.authenticated);
    }
}

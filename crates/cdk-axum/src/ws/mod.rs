use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::{close_code, CloseFrame, Message, WebSocket};
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
use serde::Deserialize;
use serde_json::error::Category;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{timeout, MissedTickBehavior};

use crate::MintState;

mod budget;
mod error;
mod limits;
mod subscribe;
mod unsubscribe;

pub(crate) use budget::{Charge, RequestBudget};
pub(crate) use limits::{WsConnectionGuard, WsConnectionLimiter};
pub use limits::{WsLimits, WsLimitsError, WsLimitsField};

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
///
/// The version is checked here as well as in [`classify_request`] because
/// `jsonrpc` is an ordinary `String` on the wire type: a frame naming another
/// version still deserializes when the rest of it is well formed, and would
/// otherwise reach the handler without ever passing the classifier.
fn deserialize_request(text: &str) -> Result<WsRequest, Rejection> {
    let err = match serde_json::from_str::<WsRequest>(text) {
        Ok(request) if request.jsonrpc == JSON_RPC_VERSION => return Ok(request),
        Ok(_) => return Err(Rejection::Answered(WsError::InvalidRequest, None)),
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

/// Only the JSON-RPC id, used to answer a frame whose body was never parsed.
#[derive(Deserialize)]
struct RequestEnvelope {
    #[serde(default)]
    id: Option<usize>,
}

/// Recovers the request id while skipping every other value, so a frame refused
/// by the connection's budget is never walked as a full `serde_json::Value`.
///
/// This is the throttled path only. A frame that is parsed gets its id from
/// [`classify_request`], which can also tell a notification from a bad envelope.
fn recover_request_id(text: &str) -> Option<usize> {
    match serde_json::from_str::<RequestEnvelope>(text) {
        Ok(envelope) => envelope.id,
        Err(err) => {
            tracing::debug!("Could not recover a request id from a refused frame: {err}");
            None
        }
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
    /// Raised by a subscription that could not hand an event to the writer
    /// before its deadline.
    ///
    /// The event cannot simply be dropped: a client that misses the final state
    /// transition of a quote has no way to tell it is stale. Closing instead
    /// makes the failure something the client can see and recover from by
    /// resubscribing.
    delivery_failed: mpsc::Sender<()>,
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

/// Why a frame could not be handed to the peer.
#[derive(Debug)]
enum SendFailure {
    /// The peer stopped reading and the write never drained.
    Timeout(Duration),
    /// The socket itself failed.
    Socket(axum::Error),
}

impl fmt::Display for SendFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout(after) => {
                write!(f, "write timed out after {:.1}s", after.as_secs_f32())
            }
            Self::Socket(err) => write!(f, "{err}"),
        }
    }
}

/// Sends one frame, giving up once `deadline` passes.
///
/// A peer that stops reading would otherwise leave the write stalled inside the
/// select arm, where the idle check never runs and the connection keeps its
/// slot for as long as the peer refuses to read. The idle timeout is reused as
/// the deadline: a write that cannot drain in that long is as dead as a
/// connection that says nothing.
async fn send(
    socket: &mut WebSocket,
    message: Message,
    deadline: Duration,
) -> Result<(), SendFailure> {
    match timeout(deadline, socket.send(message)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(SendFailure::Socket(err)),
        Err(_elapsed) => Err(SendFailure::Timeout(deadline)),
    }
}

/// Builds a close frame.
fn close_message(code: u16, reason: &'static str) -> Message {
    Message::Close(Some(CloseFrame {
        code,
        reason: reason.into(),
    }))
}

/// Decodes a binary frame as request text.
///
/// Clients are expected to send text; a binary frame is read as a request only
/// when it happens to carry UTF-8.
fn decode_request(bin: &[u8]) -> Option<&str> {
    match std::str::from_utf8(bin) {
        Ok(text) => Some(text),
        Err(err) => {
            tracing::debug!("Could not decode request: {err}");
            None
        }
    }
}

/// What one inbound frame leaves the read loop to do.
#[derive(Debug)]
enum FrameOutcome {
    /// Answer the client with this response.
    Respond(serde_json::Value),
    /// The frame needs no answer.
    Quiet,
    /// The response could not be serialized, so the connection cannot continue.
    Failed(serde_json::Error),
    /// The connection has been throttled once too often; close it.
    Exhausted,
}

fn respond(result: Result<serde_json::Value, serde_json::Error>) -> FrameOutcome {
    match result {
        Ok(response) => FrameOutcome::Respond(response),
        Err(error) => FrameOutcome::Failed(error),
    }
}

/// Units a parsed request costs on top of the one its frame was already
/// charged.
///
/// A subscription pays one unit per filter, because each filter registers a
/// topic under the mint-wide lock that event delivery also needs.
fn request_units(request: &WsRequest) -> u32 {
    match &request.method {
        WsMethodRequest::Subscribe(params) => {
            u32::try_from(params.filters.len()).unwrap_or(u32::MAX)
        }
        WsMethodRequest::Unsubscribe(_) => 0,
    }
}

/// Charges one inbound frame against the connection's budget and dispatches it.
///
/// Every charge a connection makes runs here, so a budget exhausted at either
/// stage closes the socket instead of being answered forever: a client can keep
/// the one-unit frame charge affordable while asking for far more work than it
/// can pay for, and only the second charge sees that.
///
/// `text` is `None` for a frame that carries no request, which is charged like
/// any other so a flood of control frames still costs the connection. A parsed
/// request is charged before it is judged, because the mint has already read
/// every filter by the time it can tell whether it will accept them.
async fn handle_frame(context: &mut WsContext, text: Option<&str>, now: Instant) -> FrameOutcome {
    match context.budget.charge(1, now) {
        Charge::Accepted => {}
        Charge::Throttled => {
            return match text.and_then(recover_request_id) {
                Some(id) => respond(error_response(Some(id), WsError::ServerBusy)),
                None => FrameOutcome::Quiet,
            }
        }
        Charge::Exhausted => return FrameOutcome::Exhausted,
    }

    let Some(text) = text else {
        return FrameOutcome::Quiet;
    };

    let request = match deserialize_request(text) {
        Ok(request) => request,
        Err(Rejection::Ignored) => return FrameOutcome::Quiet,
        Err(Rejection::Answered(err, request_id)) => {
            tracing::debug!("Rejected ws request: {err:?}");
            return respond(error_response(request_id, err));
        }
    };

    match context.budget.charge(request_units(&request), now) {
        Charge::Accepted => respond(process(context, request).await),
        Charge::Throttled => {
            tracing::debug!("WebSocket request exceeds the connection's request budget");
            respond(error_response(Some(request.id), WsError::ServerBusy))
        }
        Charge::Exhausted => FrameOutcome::Exhausted,
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
    let limits = state.ws_limiter.limits().clone();
    let (publisher, mut subscriber) = mpsc::channel(100);
    let (delivery_failed, mut delivery_failures) = mpsc::channel(1);
    let started_at = Instant::now();
    let mut context = WsContext {
        state,
        subscriptions: HashMap::new(),
        topics_in_use: 0,
        budget: RequestBudget::new(&limits, started_at),
        publisher,
        delivery_failed,
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

                if let Err(err) = send(
                    &mut socket,
                    Message::Text(message.into()),
                    limits.idle_timeout,
                ).await {
                    tracing::error!("Could not send websocket message: {}", err);
                    break;
                }
            }

            Some(()) = delivery_failures.recv() => {
                tracing::debug!("ws-slow: closing, a notification could not be delivered in time");
                if let Err(err) = send(
                    &mut socket,
                    close_message(close_code::AGAIN, "notification delivery timed out"),
                    limits.idle_timeout,
                ).await {
                    tracing::debug!("Could not send delivery close frame: {err}");
                }
                break;
            }

            _ = keepalive.tick() => {
                if last_activity.elapsed() >= limits.idle_timeout {
                    tracing::debug!("ws-idle: closing after {:?}", last_activity.elapsed());
                    if let Err(err) = send(
                        &mut socket,
                        close_message(close_code::POLICY, "idle timeout"),
                        limits.idle_timeout,
                    ).await {
                        tracing::debug!("Could not send idle close frame: {err}");
                    }
                    break;
                }

                if let Err(err) = send(
                    &mut socket,
                    Message::Ping(Default::default()),
                    limits.idle_timeout,
                ).await {
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

                // Any inbound frame proves the peer is alive, the pong answering
                // our own keepalive ping included, so the idle clock restarts
                // before the frame is judged on its contents.
                let now = Instant::now();
                last_activity = now;

                let text = match &message {
                    Message::Text(text) => Some(text.as_str()),
                    Message::Binary(bin) => decode_request(bin),
                    // Axum answers pings itself; replying here too would send a
                    // second pong for every ping a client sends.
                    Message::Ping(_) | Message::Pong(_) => None,
                    Message::Close(frame) => {
                        if let Some(CloseFrame { code, reason }) = frame {
                            tracing::info!(
                                "ws-close: code={code:?} reason='{}'",
                                escape_control(reason)
                            );
                        } else {
                            tracing::info!("ws-close: no frame");
                        }

                        if let Err(err) = send(
                            &mut socket,
                            close_message(close_code::NORMAL, "bye!"),
                            limits.idle_timeout,
                        ).await {
                            tracing::debug!("Could not send close frame: {err}");
                        }
                        break;
                    }
                };

                match handle_frame(&mut context, text, now).await {
                    FrameOutcome::Respond(response) => {
                        if let Err(err) = send(
                            &mut socket,
                            Message::Text(response.to_string().into()),
                            limits.idle_timeout,
                        ).await {
                            tracing::debug!("Could not send request: {}", err);
                            break;
                        }
                    }
                    FrameOutcome::Quiet => {}
                    FrameOutcome::Failed(err) => {
                        tracing::error!("Error serializing response: {}", err);
                        break;
                    }
                    FrameOutcome::Exhausted => {
                        tracing::debug!("ws-rate: closing after repeated throttling");
                        if let Err(err) = send(
                            &mut socket,
                            close_message(close_code::POLICY, "request rate exceeded"),
                            limits.idle_timeout,
                        ).await {
                            tracing::debug!("Could not send rate-limit close frame: {err}");
                        }
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
    use cdk::nuts::nut17::{MAX_CUSTOM_KIND_LEN, MAX_FILTER_LEN, MAX_SUBSCRIPTION_ID_LEN};
    use cdk::nuts::{CurrencyUnit, MintInfo};
    use cdk::subscription::{Params, SubId};
    use cdk::ws::WsUnsubscribeRequest;
    use cdk_common::pub_sub::PubsubLimits;
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

    /// These reach the version check through `classify_request`, since each also
    /// fails to deserialize.
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

    /// A frame that deserializes cleanly never reaches `classify_request`, so an
    /// unsupported version has to be caught on the happy path as well.
    #[test]
    fn an_unsupported_version_with_valid_params_is_rejected() {
        for request in [
            serde_json::json!({
                "jsonrpc": "1.0",
                "method": "unsubscribe",
                "params": { "subId": "sub-1" },
                "id": 5,
            }),
            serde_json::json!({
                "jsonrpc": "1.0",
                "method": "subscribe",
                "params": {
                    "kind": "bolt11_mint_quote",
                    "filters": ["quote-id"],
                    "subId": "sub-1",
                },
                "id": 6,
            }),
        ] {
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
        let ws_limiter = Arc::new(WsConnectionLimiter::new(limits.clone()));
        let connection_guard = ws_limiter.try_acquire().expect("connection slot");
        let state = MintState {
            mint,
            cache: Arc::new(HttpCache::default()),
            ws_limiter,
        };
        let (publisher, _receiver) = tokio::sync::mpsc::channel(100);
        let (delivery_failed, _failures) = tokio::sync::mpsc::channel(1);
        WsContext {
            state,
            subscriptions: HashMap::new(),
            topics_in_use: 0,
            budget: RequestBudget::new(&limits, Instant::now()),
            publisher,
            delivery_failed,
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
    fn make_params_with_filters(sub_id: &str, filters: usize) -> Params {
        Params {
            kind: cdk::nuts::nut17::Kind::Bolt11MintQuote,
            filters: (0..filters).map(|_| QuoteId::new().to_string()).collect(),
            id: Arc::new(SubId::from(sub_id)),
        }
    }

    fn subscribe_frame(id: usize, sub_id: &str, filters: usize) -> String {
        serde_json::to_string(&WsRequest::from((
            WsMethodRequest::Subscribe(make_params_with_filters(sub_id, filters)),
            id,
        )))
        .expect("subscribe frame")
    }

    fn unsubscribe_frame(id: usize, sub_id: &str) -> String {
        serde_json::to_string(&WsRequest::from((
            WsMethodRequest::Unsubscribe(WsUnsubscribeRequest {
                sub_id: Arc::new(SubId::from(sub_id)),
            }),
            id,
        )))
        .expect("unsubscribe frame")
    }

    /// The response body a frame produced, or a panic naming what came instead.
    fn responded(outcome: FrameOutcome) -> serde_json::Value {
        match outcome {
            FrameOutcome::Respond(response) => response,
            other => panic!("expected a response, got {other:?}"),
        }
    }

    async fn frame(context: &mut WsContext, text: &str) -> serde_json::Value {
        responded(handle_frame(context, Some(text), Instant::now()).await)
    }

    /// `WsError::ServerBusy` on the wire.
    const SERVER_BUSY: i64 = -32000;

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

        let response = frame(&mut context, &subscribe_frame(1, "sub-a", 8)).await;

        assert_eq!(
            response["error"]["code"], SERVER_BUSY,
            "subscription over the connection's request budget should be rejected as busy"
        );
        assert_eq!(
            context.topics_in_use, 0,
            "rejection must not consume the topic budget"
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

    /// The smallest burst the mintd config validation accepts is one unit for
    /// the frame plus one per filter, so that burst must actually admit a
    /// maximum-size subscription.
    #[tokio::test]
    async fn the_minimum_burst_admits_a_maximum_size_subscription() {
        let filters = 8;
        let mint = create_test_mint().await;
        let limits = WsLimits {
            max_filters_per_subscription: filters,
            max_topics_per_connection: filters,
            max_request_units_per_second: 1,
            max_request_burst_units: u32::try_from(filters).expect("filter count") + 1,
            max_throttled_requests: 0,
            ..WsLimits::default()
        };
        limits
            .validate()
            .expect("the smallest burst the validation accepts");
        let mut context = make_context_with_limits(mint, limits);

        let response = frame(&mut context, &subscribe_frame(1, "sub-max", filters)).await;

        assert_eq!(
            response["result"]["status"], "OK",
            "the smallest valid burst must cover a maximum-size subscription: {response}"
        );
    }

    /// A subscription costs one unit per filter, so churning wide subscriptions
    /// drains the budget far faster than the per-connection topic cap alone
    /// would suggest.
    #[tokio::test]
    async fn subscribe_churn_is_charged_per_filter() {
        let mint = create_test_mint().await;
        let limits = WsLimits {
            max_request_units_per_second: 1,
            max_request_burst_units: 16,
            max_throttled_requests: 0,
            ..WsLimits::default()
        };
        let mut context = make_context_with_limits(mint, limits);

        for round in 0..2 {
            let sub_id = format!("sub-{round}");
            let subscribed = frame(&mut context, &subscribe_frame(round, &sub_id, 5)).await;
            assert_eq!(
                subscribed["result"]["status"], "OK",
                "round {round} fits in the request budget: {subscribed}"
            );

            let unsubscribed = frame(&mut context, &unsubscribe_frame(round, &sub_id)).await;
            assert_eq!(unsubscribed["result"]["status"], "OK");
        }

        let exhausted = frame(&mut context, &subscribe_frame(2, "sub-2", 5)).await;

        assert_eq!(
            exhausted["error"]["code"], SERVER_BUSY,
            "a third round of churn must exhaust the request budget: {exhausted}"
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
            let response = frame(
                &mut context,
                &subscribe_frame(round, &format!("sub-{round}"), 5),
            )
            .await;
            assert_eq!(
                response["result"]["status"], "OK",
                "a disabled throttle admits every request: {response}"
            );
        }
    }

    /// An unsubscribe costs only the unit its frame was charged, so a client
    /// tidying up after itself is not pushed towards the disconnect threshold.
    #[tokio::test]
    async fn only_a_subscription_is_charged_beyond_its_frame() {
        let subscribe: WsRequest =
            serde_json::from_str(&subscribe_frame(1, "sub-a", 7)).expect("subscribe request");
        let unsubscribe: WsRequest =
            serde_json::from_str(&unsubscribe_frame(2, "sub-a")).expect("unsubscribe request");

        assert_eq!(request_units(&subscribe), 7);
        assert_eq!(request_units(&unsubscribe), 0);
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

    /// Drives `handshakes` real handshakes over TCP, holding each socket open,
    /// and returns the status each one got: the `WebSocketUpgrade` extractor
    /// needs hyper's upgrade extension, which an in-memory request does not
    /// carry.
    async fn handshake_status(limits: WsLimits, handshakes: usize) -> Vec<u16> {
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
            if let Err(err) = axum::serve(listener, router.into_make_service()).await {
                tracing::debug!("test server stopped: {err}");
            }
        });

        let mut statuses = Vec::new();
        // Held open so each handshake sees the slots the previous ones took.
        let mut sockets = Vec::new();

        for _ in 0..handshakes {
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
                ..WsLimits::default()
            },
            2,
        )
        .await;

        assert_eq!(statuses, vec![101, 503]);
    }

    /// A context whose writer queue holds one event and is never drained, plus
    /// the channel its subscriptions raise a delivery failure on.
    type StalledContext = (
        WsContext,
        mpsc::Receiver<(Arc<SubId>, NotificationPayload<QuoteId>)>,
        mpsc::Receiver<()>,
    );

    fn make_stalled_context(mint: Arc<Mint>, limits: WsLimits) -> StalledContext {
        let ws_limiter = Arc::new(WsConnectionLimiter::new(limits.clone()));
        let connection_guard = ws_limiter.try_acquire().expect("connection slot");
        let state = MintState {
            mint,
            cache: Arc::new(HttpCache::default()),
            ws_limiter,
        };
        let (publisher, receiver) = mpsc::channel(1);
        let (delivery_failed, failures) = mpsc::channel(1);
        let context = WsContext {
            state,
            subscriptions: HashMap::new(),
            topics_in_use: 0,
            budget: RequestBudget::new(&limits, Instant::now()),
            publisher,
            delivery_failed,
            _connection_guard: connection_guard,
        };

        (context, receiver, failures)
    }

    /// A notification that cannot reach the writer must take the connection
    /// down rather than be dropped: the event could be the final state
    /// transition of a quote, and a client that misses it has no way to tell it
    /// is stale.
    #[tokio::test]
    async fn an_undeliverable_notification_closes_the_connection() {
        use cdk::nuts::{ProofState, PublicKey, State};

        let delivery_timeout = Duration::from_millis(200);
        let limits = WsLimits {
            ping_interval: delivery_timeout / 2,
            idle_timeout: delivery_timeout,
            ..WsLimits::default()
        };
        let mint = create_test_mint().await;
        let pubsub = mint.pubsub_manager();
        let (mut context, _queue, mut failures) = make_stalled_context(mint, limits);

        let y = PublicKey::from_hex(
            "02194603ffa36356f4a56b7df9371fc3192472351453ec7398b8da8117e7c3e104",
        )
        .expect("public key");

        subscribe::handle(
            &mut context,
            Params {
                kind: cdk::nuts::nut17::Kind::ProofState,
                filters: vec![y.to_hex()],
                id: Arc::new(SubId::from("stalled")),
            },
        )
        .await
        .expect("subscribe");

        // The queue is never read, so the first event fills it and the second
        // has nowhere to go.
        for state in [State::Pending, State::Spent] {
            pubsub.proof_state(ProofState {
                y,
                state,
                witness: None,
            });
        }

        timeout(delivery_timeout * 10, failures.recv())
            .await
            .expect("the connection must be told before the deadline is long past")
            .expect("a delivery failure must be raised");
    }

    /// Serves the WebSocket route on a loopback port and connects a real client
    /// to it, so a test drives the same path a client does: the handshake, the
    /// read loop, and every charge either stage makes.
    async fn ws_client(
        limits: WsLimits,
    ) -> (
        tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
        tokio::task::JoinHandle<()>,
    ) {
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

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let server = tokio::spawn(async move {
            if let Err(err) = axum::serve(listener, router.into_make_service()).await {
                tracing::debug!("test server stopped: {err}");
            }
        });

        let stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
        let (client, _) = tokio_tungstenite::client_async(format!("ws://{addr}/v1/ws"), stream)
            .await
            .expect("websocket handshake");

        (client, server)
    }

    /// Sends one frame and reads the answer to it.
    async fn round_trip(
        client: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
        request: String,
    ) -> serde_json::Value {
        use futures::SinkExt;

        client
            .send(tokio_tungstenite::tungstenite::Message::Text(
                request.into(),
            ))
            .await
            .expect("send frame");

        let message = timeout(Duration::from_secs(10), client.next())
            .await
            .expect("response before timeout")
            .expect("stream open")
            .expect("websocket message");

        match message {
            tokio_tungstenite::tungstenite::Message::Text(text) => {
                serde_json::from_str(&text).expect("json response")
            }
            other => panic!("unexpected frame: {other:?}"),
        }
    }

    /// The frame charge stays affordable long after the filter charge stops
    /// being so, which is how a client could once be refused forever without
    /// ever reaching the disconnect threshold. The first subscribe and its
    /// unsubscribe drain the burst to where one more frame still fits but the
    /// filters it carries no longer do.
    #[tokio::test]
    async fn repeated_filter_budget_violations_close_the_connection() {
        use futures::SinkExt;

        let filters = 10;
        let limits = WsLimits {
            max_filters_per_subscription: filters,
            max_topics_per_connection: filters,
            max_request_units_per_second: 1,
            max_request_burst_units: 20,
            max_throttled_requests: 3,
            ..WsLimits::default()
        };
        limits.validate().expect("servable limits");

        let (mut client, server) = ws_client(limits).await;

        let subscribed = round_trip(&mut client, subscribe_frame(0, "sub-0", filters)).await;
        assert_eq!(subscribed["result"]["status"], "OK", "{subscribed}");
        let unsubscribed = round_trip(&mut client, unsubscribe_frame(1, "sub-0")).await;
        assert_eq!(unsubscribed["result"]["status"], "OK", "{unsubscribed}");

        for id in 2..4 {
            let refused = round_trip(
                &mut client,
                subscribe_frame(id, &format!("sub-{id}"), filters),
            )
            .await;
            assert_eq!(
                refused["error"]["code"], SERVER_BUSY,
                "request {id} is over the filter budget: {refused}"
            );
        }

        client
            .send(tokio_tungstenite::tungstenite::Message::Text(
                subscribe_frame(4, "sub-4", filters).into(),
            ))
            .await
            .expect("send frame");

        let closed = timeout(Duration::from_secs(10), client.next())
            .await
            .expect("close before timeout")
            .expect("stream open")
            .expect("websocket message");

        assert!(
            matches!(
                closed,
                tokio_tungstenite::tungstenite::Message::Close(Some(ref frame))
                    if frame.code == tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Policy
            ),
            "one violation past the threshold must close the connection, got {closed:?}"
        );

        server.abort();
    }

    /// The idle check and the ping share one timer tick, and the check runs
    /// first, so a quiet connection only survives if the ping arrives on an
    /// earlier tick than the one that would close it. This drives that over a
    /// real socket: the client answers nothing itself, it only lets the
    /// library's automatic pong go out.
    #[tokio::test]
    async fn a_quiet_connection_is_pinged_before_its_idle_timeout() {
        let idle_timeout = Duration::from_millis(300);
        let limits = WsLimits {
            ping_interval: Duration::from_millis(100),
            idle_timeout,
            ..WsLimits::default()
        };
        limits.validate().expect("servable limits");

        let (mut client, server) = ws_client(limits).await;

        let mut pings = 0;
        let until = tokio::time::Instant::now() + idle_timeout * 3;
        while tokio::time::Instant::now() < until {
            match timeout(Duration::from_millis(50), client.next()).await {
                Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Ping(_)))) => pings += 1,
                Err(_) => continue,
                other => panic!("a silent but healthy connection must stay open, got {other:?}"),
            }
        }

        assert!(
            pings >= 2,
            "the connection should have been pinged repeatedly, saw {pings}"
        );

        server.abort();
    }

    /// A zero ping interval panics `tokio::time::interval`, so the router must not
    /// be handed out at all.
    #[tokio::test]
    async fn a_router_is_not_built_from_limits_that_cannot_be_served() {
        let err = crate::create_mint_router_with_custom_cache(
            create_test_mint().await,
            HttpCache::default(),
            Vec::new(),
            false,
            WsLimits {
                ping_interval: Duration::ZERO,
                ..WsLimits::default()
            },
        )
        .await
        .expect_err("limits that cannot be served must not yield a router");

        assert_eq!(
            err.downcast_ref::<WsLimitsError>(),
            Some(&WsLimitsError::Zero {
                field: WsLimitsField::PingInterval
            }),
            "unexpected error: {err:#}"
        );
    }
}

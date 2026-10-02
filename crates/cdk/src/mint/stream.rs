//! Transport-agnostic NUT-17 subscription runner.
//!
//! [`run_stream`] drives the NUT-17 subscribe/notify protocol over a raw
//! [`StreamTx`] / [`StreamRx`] duplex against a [`Mint`], independent of how the
//! stream is carried. `MintServer::open_stream` uses it over an in-memory duplex;
//! a network transport (its adapter) can bridge an accepted socket to the same
//! halves and reuse it.

use std::collections::HashMap;
use std::sync::Arc;

use cdk_common::nut17::ws::{WsErrorResponse, JSON_RPC_VERSION};
use cdk_common::nut17::{
    NotificationPayload, MAX_CUSTOM_KIND_LEN, MAX_FILTER_LEN, MAX_SUBSCRIPTION_ID_LEN,
};
use cdk_common::pub_sub::Error as PubSubError;
use cdk_common::stream_channel::{StreamRx, StreamTx};
use cdk_common::subscription::SubId;
use cdk_common::terminal::escape_control;
use cdk_common::ws::{
    notification_to_ws_message, NotificationInner, WsErrorBody, WsMessageOrResponse,
    WsMethodRequest, WsRequest, WsResponseResult,
};
use serde_json::error::Category;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio::task::JoinHandle;

use super::{Mint, QuoteId};

const MAX_SUBSCRIPTIONS_PER_CONNECTION: usize = 100;
const MAX_FILTERS_PER_SUBSCRIPTION: usize = 1000;

/// Room for the JSON-RPC envelope around a maximal subscribe: method name,
/// field keys, brackets and separators.
const WS_ENVELOPE_SLACK: usize = 1024;

/// Byte size of the largest subscribe request this runner will accept, from the
/// limits it enforces: every filter at [`MAX_FILTER_LEN`] plus its quotes and
/// comma, a maximal `subId` and a maximal custom `kind`.
const MAX_LEGAL_REQUEST_SIZE: usize = MAX_FILTERS_PER_SUBSCRIPTION * (MAX_FILTER_LEN + 3)
    + MAX_SUBSCRIPTION_ID_LEN
    + MAX_CUSTOM_KIND_LEN
    + WS_ENVELOPE_SLACK;

/// Largest NUT-17 frame a transport should accept before the runner sees it.
///
/// Transports (the `cdk-axum` websocket adapter) clamp their reassembly buffer
/// to this so an unbounded frame cannot be allocated on the mint's behalf. The
/// assertion below ties it to the subscribe limits above, so tightening the cap
/// can never silently start rejecting requests `handle_request` would accept.
pub const MAX_WS_MESSAGE_SIZE: usize = 512 * 1024;

const _: () = assert!(
    MAX_WS_MESSAGE_SIZE >= MAX_LEGAL_REQUEST_SIZE,
    "MAX_WS_MESSAGE_SIZE would reject a protocol-legal subscribe"
);

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
                let response: WsMessageOrResponse = match deserialize_request(&text) {
                    Ok(request) => {
                        let id = request.id;
                        let result = handle_request(&mint, &publisher, &mut subscriptions, request)
                            .await
                            .map_err(WsErrorBody::from);
                        (id, result).into()
                    }
                    Err(Rejection::Ignored) => continue,
                    Err(Rejection::Answered(err, request_id)) => {
                        tracing::error!("Rejected ws request: {err:?}");
                        error_response(request_id, err)
                    }
                };
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

/// A JSON-RPC error this runner answers a rejected request with.
///
/// Source: <https://www.jsonrpc.org/specification#error_object>
#[derive(Debug)]
enum WsError {
    /// Invalid JSON was received by the server.
    ParseError,
    /// The JSON sent is not a valid Request object.
    InvalidRequest,
    /// The method does not exist / is not available.
    MethodNotFound,
    /// Invalid method parameter(s).
    InvalidParams,
    /// Internal JSON-RPC error.
    InternalError,
}

impl From<WsError> for WsErrorBody {
    fn from(val: WsError) -> Self {
        let (code, message) = match val {
            WsError::ParseError => (-32700, "Parse error".to_string()),
            WsError::InvalidRequest => (-32600, "Invalid Request".to_string()),
            WsError::MethodNotFound => (-32601, "Method not found".to_string()),
            WsError::InvalidParams => (-32602, "Invalid params".to_string()),
            WsError::InternalError => (-32603, "Internal error".to_string()),
        };
        WsErrorBody { code, message }
    }
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

/// Build the error reply for a rejected frame, whose id may not have survived
/// parsing. JSON-RPC 2.0 wants the `id` member present and null in that case,
/// which the `(usize, Result<..>)` conversion cannot express.
fn error_response(request_id: Option<usize>, error: WsError) -> WsMessageOrResponse {
    WsErrorResponse::new(request_id, WsErrorBody::from(error)).into()
}

async fn handle_request(
    mint: &Mint,
    publisher: &mpsc::Sender<(Arc<SubId>, NotificationPayload<QuoteId>)>,
    subscriptions: &mut Subscriptions,
    request: WsRequest,
) -> Result<WsResponseResult, WsError> {
    match request.method {
        WsMethodRequest::Subscribe(params) => {
            let sub_id = params.id.clone();
            if subscriptions.contains(&sub_id) {
                return Err(WsError::InvalidParams);
            }
            if subscriptions.len() >= MAX_SUBSCRIPTIONS_PER_CONNECTION {
                tracing::warn!(
                    "subscription request exceeds per-connection limit: {} >= {}",
                    subscriptions.len(),
                    MAX_SUBSCRIPTIONS_PER_CONNECTION
                );
                return Err(WsError::InvalidParams);
            }
            if params.filters.len() > MAX_FILTERS_PER_SUBSCRIPTION {
                tracing::warn!(
                    "subscription request exceeds max filters limit: {} > {}",
                    params.filters.len(),
                    MAX_FILTERS_PER_SUBSCRIPTION
                );
                return Err(WsError::InvalidParams);
            }

            let mut subscription =
                mint.pubsub_manager()
                    .subscribe(params)
                    .map_err(|err| match err {
                        PubSubError::ParsingError(reason) => {
                            tracing::warn!(
                                "Invalid NUT-17 subscription params: {}",
                                escape_control(&reason)
                            );
                            WsError::InvalidParams
                        }
                        other => {
                            tracing::error!("Could not subscribe: {other}");
                            WsError::InternalError
                        }
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
            None => Err(WsError::InvalidParams),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use cdk_common::nut17::Kind;
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

    fn rejection_of(text: &str) -> serde_json::Value {
        match deserialize_request(text).expect_err("a rejected request") {
            Rejection::Answered(err, request_id) => {
                serde_json::to_value(error_response(request_id, err)).expect("error response")
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
        for text in ["{not json", "", "[1, 2", "\u{feff}{}", "{\"jsonrpc\": }"] {
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

    /// Guards the transport cap against being tightened below what the runner
    /// accepts: a subscribe at every NUT-17 limit must still fit in one frame.
    #[test]
    fn maximal_legal_subscribe_fits_the_transport_cap() {
        let params = Params {
            kind: Kind::Custom("k".repeat(MAX_CUSTOM_KIND_LEN)),
            filters: (0..MAX_FILTERS_PER_SUBSCRIPTION)
                .map(|_| "f".repeat(MAX_FILTER_LEN))
                .collect(),
            id: Arc::new(SubId::from("a".repeat(MAX_SUBSCRIPTION_ID_LEN))),
        };
        let frame =
            serde_json::to_string(&WsRequest::from((WsMethodRequest::Subscribe(params), 0)))
                .expect("a serializable subscribe");

        assert!(
            frame.len() <= MAX_WS_MESSAGE_SIZE,
            "a protocol-legal subscribe is {} bytes, over the {MAX_WS_MESSAGE_SIZE} byte cap",
            frame.len()
        );
        deserialize_request(&frame).expect("a maximal subscribe is a valid request");
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

    /// A filter the mint cannot parse clears deserialization, so only
    /// `pubsub_manager().subscribe` can reject it; that rejection is the
    /// client's fault and must not be reported as an internal error.
    #[tokio::test]
    async fn unparsable_filter_is_invalid_params_not_internal_error() {
        let mint = create_test_mint().await.unwrap();
        let ((mut tx, mut rx), runner) = spawn_runner(&mint);

        let params = Params {
            kind: Kind::Bolt11MintQuote,
            filters: vec!["not-a-quote-id".to_string()],
            id: Arc::new(SubId::from("bad-filter")),
        };
        let frame =
            serde_json::to_string(&WsRequest::from((WsMethodRequest::Subscribe(params), 7)))
                .unwrap();
        tx.send(frame).await.unwrap();

        let reply = expect_reply(&mut rx).await;
        let reply: serde_json::Value = serde_json::from_str(&reply).expect("a JSON reply");
        assert_eq!(
            reply["error"]["code"],
            serde_json::json!(-32602),
            "unparsable filter should be invalid params: {reply}"
        );

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

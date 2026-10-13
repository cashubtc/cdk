//! WebSocket transport for NUT-17 subscriptions.
//!
//! The protocol itself (subscribe/unsubscribe, notification framing, per-connection
//! limits, cleanup) lives once in the shared runner `Mint::serve_stream`
//! (`cdk::mint::stream`). This module only bridges an accepted axum [`WebSocket`]
//! into the transport-neutral [`StreamTx`]/[`StreamRx`] halves the runner speaks,
//! the server-side mirror of `cdk::stream_channel::from_ws`.

use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use cdk::mint::MAX_WS_MESSAGE_SIZE;
use cdk::stream_channel::{StreamError, StreamRx, StreamTx};
use cdk_common::terminal::escape_control;
use futures::{SinkExt, StreamExt};
use tokio::time::timeout;

use crate::MintState;

/// Cap on tungstenite's outbound write buffer.
///
/// The buffer only grows past its 128 KiB working size when writes to the
/// socket are erroring, so this bounds that path rather than the ordinary
/// slow-reader one, which already pends on the socket and trips
/// [`STALL_TIMEOUT`]. Kept well above one maximal message, as tungstenite
/// requires.
pub(crate) const MAX_WS_WRITE_BUFFER: usize = 1024 * 1024;

/// Capacity of the socket-to-runner channel.
///
/// Inbound frames are attacker-sized and attacker-paced, so this queue stays
/// shallow: the bridge stops reading the socket almost as soon as the runner
/// falls behind, which closes the receive window on a flooder instead of
/// buffering its frames on the heap. Kept just deep enough that
/// [`STALL_TIMEOUT`] measures a wedged runner rather than one slow request.
const INBOUND_CAP: usize = 4;

/// Capacity of the runner-to-socket channel.
///
/// Outbound notifications are mint-generated and size-bounded, so this queue can
/// absorb a burst and keep the publisher off the socket's write path.
const OUTBOUND_CAP: usize = 128;

/// Max time any bridge send (to the socket or to the runner) may block before we
/// treat the peer/runner as stalled and tear the connection down, so a client
/// that stops reading (or floods requests) cannot pin the task indefinitely.
const STALL_TIMEOUT: Duration = Duration::from_secs(30);

/// Best-effort budget for the teardown Close frame; the peer is likely already
/// gone, so we never block long on it.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);

/// Apply the transport limits every NUT-17 socket runs under.
///
/// Clamping at the upgrade is the only place that bounds a frame: tungstenite
/// reassembles fragments into one buffer before [`bridge`] ever sees a message,
/// so no channel capacity can stop an oversized allocation.
pub(crate) fn configure(ws: WebSocketUpgrade) -> WebSocketUpgrade {
    ws.max_message_size(MAX_WS_MESSAGE_SIZE)
        .max_frame_size(MAX_WS_MESSAGE_SIZE)
        .max_write_buffer_size(MAX_WS_WRITE_BUFFER)
}

/// Gate the upgraded socket onto the shared NUT-17 runner.
///
/// Auth (NUT-21/22) is already verified by `ws_handler` before the upgrade, so
/// this only wires transport to protocol.
pub(crate) async fn serve(socket: WebSocket, state: MintState) {
    let (tx, rx) = bridge(socket);
    state.mint.serve_stream(tx, rx).await;
}

/// Bridge an accepted [`WebSocket`] into transport-neutral stream halves.
///
/// The runner speaks only `String` messages, so a task owns the socket and
/// translates frames: Text/Binary become inbound `String`s, Ping is answered
/// with Pong (axum does not auto-reply), Pong is ignored, and Close or a
/// transport error ends the stream.
///
/// NUT-17 frames are JSON, so a Binary payload that is not UTF-8 is malformed
/// and tears the connection down instead of reaching the parser as the mangled
/// text a lossy conversion would produce.
fn bridge(mut socket: WebSocket) -> (StreamTx, StreamRx) {
    // outbound: the runner's StreamTx -> socket
    let (out_tx, mut out_rx) = futures::channel::mpsc::channel::<String>(OUTBOUND_CAP);
    // inbound: socket -> the runner's StreamRx
    let (mut in_tx, in_rx) =
        futures::channel::mpsc::channel::<Result<String, StreamError>>(INBOUND_CAP);

    tokio::spawn(async move {
        // One task owns both directions of the socket, so a stall in either arm
        // is bounded by STALL_TIMEOUT rather than pinning the task. The loop
        // yields whether a teardown Close frame is still owed.
        let send_close = loop {
            tokio::select! {
                outbound = out_rx.next() => {
                    match outbound {
                        Some(message) => {
                            match timeout(STALL_TIMEOUT, socket.send(Message::Text(message.into()))).await {
                                Ok(Ok(())) => {}
                                // Socket error or an elapsed timeout (client not
                                // draining): tear down either way.
                                Ok(Err(err)) => {
                                    tracing::warn!("ws-send: socket send failed: {err}");
                                    break true;
                                }
                                Err(_) => {
                                    tracing::warn!("ws-send: client stalled for {STALL_TIMEOUT:?}");
                                    break true;
                                }
                            }
                        }
                        // Runner dropped its StreamTx: nothing more to send.
                        None => break true,
                    }
                }
                inbound = socket.next() => {
                    match inbound {
                        Some(Ok(Message::Text(text))) => {
                            match timeout(STALL_TIMEOUT, in_tx.send(Ok(text.to_string()))).await {
                                Ok(Ok(())) => {}
                                // Closed channel or an elapsed timeout (runner not
                                // draining): tear down either way.
                                Ok(Err(err)) => {
                                    tracing::warn!("ws-recv: runner channel closed: {err}");
                                    break true;
                                }
                                Err(_) => {
                                    tracing::warn!("ws-recv: runner stalled for {STALL_TIMEOUT:?}");
                                    break true;
                                }
                            }
                        }
                        Some(Ok(Message::Binary(bin))) => {
                            let text = match String::from_utf8(bin.into()) {
                                Ok(text) => text,
                                Err(err) => {
                                    tracing::warn!("ws-recv: binary frame is not utf-8: {err}");
                                    break true;
                                }
                            };
                            match timeout(STALL_TIMEOUT, in_tx.send(Ok(text))).await {
                                Ok(Ok(())) => {}
                                Ok(Err(err)) => {
                                    tracing::warn!("ws-recv: runner channel closed: {err}");
                                    break true;
                                }
                                Err(_) => {
                                    tracing::warn!("ws-recv: runner stalled for {STALL_TIMEOUT:?}");
                                    break true;
                                }
                            }
                        }
                        Some(Ok(Message::Ping(payload))) => {
                            match timeout(STALL_TIMEOUT, socket.send(Message::Pong(payload))).await {
                                Ok(Ok(())) => {}
                                Ok(Err(err)) => {
                                    tracing::warn!("ws-pong: socket send failed: {err}");
                                    break true;
                                }
                                Err(_) => {
                                    tracing::warn!("ws-pong: client stalled for {STALL_TIMEOUT:?}");
                                    break true;
                                }
                            }
                        }
                        Some(Ok(Message::Pong(_))) => {}
                        Some(Ok(Message::Close(frame))) => {
                            if let Some(CloseFrame { code, reason }) = frame {
                                tracing::info!(
                                    "ws-close: code={code:?} reason='{}'",
                                    escape_control(&reason)
                                );
                            }
                            // Reading the peer's Close already queued tungstenite's
                            // reply; flush it so the peer sees a clean handshake. A
                            // manual `send(Close)` here would fail with
                            // `SendAfterClosing` (state is no longer active) and skip
                            // the flush, dropping the socket with the reply unsent and
                            // leaving the peer to observe a connection reset.
                            match timeout(CLOSE_TIMEOUT, socket.flush()).await {
                                Ok(Ok(())) => {}
                                Ok(Err(err)) => tracing::debug!(
                                    "ws-close: flushing the close reply failed: {err}"
                                ),
                                Err(_) => tracing::debug!(
                                    "ws-close: flushing the close reply timed out after {CLOSE_TIMEOUT:?}"
                                ),
                            }
                            // Peer initiated close and we replied; nothing owed.
                            break false;
                        }
                        Some(Err(err)) => {
                            tracing::error!("ws-error: {err}");
                            // Socket is already broken; a Close frame won't land.
                            break false;
                        }
                        // Socket reached EOF; nothing to send.
                        None => break false,
                    }
                }
            }
        };
        // On a stall/error teardown the peer may still be live, so send a
        // graceful Close before dropping the socket. Bounded so a stalled peer
        // cannot pin the task.
        if send_close {
            match timeout(
                CLOSE_TIMEOUT,
                socket.send(Message::Close(Some(CloseFrame {
                    code: axum::extract::ws::close_code::NORMAL,
                    reason: "closing".into(),
                }))),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(err)) => tracing::debug!("ws-close: sending the close frame failed: {err}"),
                Err(_) => {
                    tracing::debug!("ws-close: close frame timed out after {CLOSE_TIMEOUT:?}")
                }
            }
        }
        // Dropping `in_tx` ends the runner's StreamRx, so it tears down and
        // aborts its subscriptions.
    });

    let tx = StreamTx::new(out_tx.sink_map_err(|e| StreamError::Send(e.to_string())));
    let rx = StreamRx::new(in_rx);
    (tx, rx)
}

#[cfg(test)]
mod tests {
    use axum::routing::get;
    use axum::Router;
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::Message as ClientMessage;
    use tokio_tungstenite::{connect_async, tungstenite};

    use super::*;

    /// Echo the bridged stream back, so a test exercises the real `configure`
    /// limits and the real `bridge` frame translation without a mint.
    async fn echo_handler(ws: WebSocketUpgrade) -> axum::response::Response {
        configure(ws).on_upgrade(|socket| async move {
            let (mut tx, mut rx) = bridge(socket);
            while let Some(Ok(message)) = rx.recv().await {
                if tx.send(message).await.is_err() {
                    break;
                }
            }
        })
    }

    async fn echo_server() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
        let addr = listener.local_addr().expect("a local addr");
        let app = Router::new().route("/ws", get(echo_handler));
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("the server runs");
        });
        format!("ws://{addr}/ws")
    }

    #[tokio::test]
    async fn a_maximal_legal_frame_round_trips() {
        let (mut socket, _) = connect_async(echo_server().await).await.expect("connected");
        let frame = "a".repeat(MAX_WS_MESSAGE_SIZE - 1024);

        socket
            .send(ClientMessage::Text(frame.clone().into()))
            .await
            .expect("send a maximal frame");

        let echoed = socket.next().await.expect("a reply").expect("no error");
        assert_eq!(echoed, ClientMessage::Text(frame.into()));
    }

    /// The server may tear the connection down mid-write, so the refusal surfaces
    /// either as a failed send or as a reply that is not the echo.
    #[tokio::test]
    async fn an_oversized_frame_is_refused() {
        let (mut socket, _) = connect_async(echo_server().await).await.expect("connected");

        if socket
            .send(ClientMessage::Text(
                "a".repeat(MAX_WS_MESSAGE_SIZE + 1).into(),
            ))
            .await
            .is_err()
        {
            return;
        }

        let outcome = socket.next().await;
        assert!(
            !matches!(outcome, Some(Ok(ClientMessage::Text(_)))),
            "an oversized frame must not be echoed, got {outcome:?}"
        );
    }

    #[tokio::test]
    async fn a_non_utf8_binary_frame_is_refused() {
        let (mut socket, _) = connect_async(echo_server().await).await.expect("connected");

        socket
            .send(ClientMessage::Binary(vec![0xff, 0xfe, 0xfd].into()))
            .await
            .expect("send an invalid utf-8 frame");

        let outcome = socket.next().await;
        assert!(
            matches!(
                outcome,
                None | Some(Ok(ClientMessage::Close(_)))
                    | Some(Err(tungstenite::Error::ConnectionClosed))
                    | Some(Err(tungstenite::Error::Protocol(_)))
            ),
            "a non-utf8 binary frame must tear the connection down, got {outcome:?}"
        );
    }
}

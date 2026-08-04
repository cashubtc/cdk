//! WebSocket transport for NUT-17 subscriptions.
//!
//! The protocol itself (subscribe/unsubscribe, notification framing, per-connection
//! limits, cleanup) lives once in the shared runner `Mint::serve_stream`
//! (`cdk::mint::stream`). This module only bridges an accepted axum [`WebSocket`]
//! into the transport-neutral [`StreamTx`]/[`StreamRx`] halves the runner speaks,
//! the server-side mirror of `cdk::stream_channel::from_ws`.

use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use cdk::stream_channel::{StreamError, StreamRx, StreamTx};
use futures::{SinkExt, StreamExt};
use tokio::time::timeout;

use crate::MintState;

/// Capacity of the in-process channels bridging the socket and the runner.
const BRIDGE_CAP: usize = 128;

/// Max time any bridge send (to the socket or to the runner) may block before we
/// treat the peer/runner as stalled and tear the connection down, so a client
/// that stops reading (or floods requests) cannot pin the task indefinitely.
const STALL_TIMEOUT: Duration = Duration::from_secs(30);

/// Best-effort budget for the teardown Close frame; the peer is likely already
/// gone, so we never block long on it.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);

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
fn bridge(mut socket: WebSocket) -> (StreamTx, StreamRx) {
    // outbound: the runner's StreamTx -> socket
    let (out_tx, mut out_rx) = futures::channel::mpsc::channel::<String>(BRIDGE_CAP);
    // inbound: socket -> the runner's StreamRx
    let (mut in_tx, in_rx) =
        futures::channel::mpsc::channel::<Result<String, StreamError>>(BRIDGE_CAP);

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
                            let text = String::from_utf8_lossy(&bin).to_string();
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
                                tracing::info!("ws-close: code={code:?} reason='{reason}'");
                            }
                            // Reading the peer's Close already queued tungstenite's
                            // reply; flush it so the peer sees a clean handshake. A
                            // manual `send(Close)` here would fail with
                            // `SendAfterClosing` (state is no longer active) and skip
                            // the flush, dropping the socket with the reply unsent and
                            // leaving the peer to observe a connection reset.
                            if let Err(err) = socket.flush().await {
                                tracing::debug!("ws-close: flushing the close reply failed: {err}");
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

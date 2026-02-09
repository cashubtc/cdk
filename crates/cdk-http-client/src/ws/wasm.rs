//! WASM WebSocket implementation using ws_stream_wasm

use futures::{SinkExt, StreamExt};
use ws_stream_wasm::{WsMessage, WsMeta};

use super::WsError;

/// WebSocket sender half
pub struct WsSender {
    inner: futures::stream::SplitSink<ws_stream_wasm::WsStream, WsMessage>,
}

impl std::fmt::Debug for WsSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WsSender").finish_non_exhaustive()
    }
}

/// WebSocket receiver half
pub struct WsReceiver {
    inner: futures::stream::SplitStream<ws_stream_wasm::WsStream>,
}

impl std::fmt::Debug for WsReceiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WsReceiver").finish_non_exhaustive()
    }
}

impl WsSender {
    /// Send a text message over the WebSocket
    pub async fn send(&mut self, text: String) -> Result<(), WsError> {
        self.inner
            .send(WsMessage::Text(text))
            .await
            .map_err(|e| WsError::Send(e.to_string()))
    }

    /// Send a close frame
    pub async fn close(&mut self) -> Result<(), WsError> {
        self.inner
            .close()
            .await
            .map_err(|e| WsError::Send(e.to_string()))
    }
}

impl WsReceiver {
    /// Receive the next text message. Returns `None` when the connection is closed.
    /// Non-text messages are silently skipped.
    pub async fn recv(&mut self) -> Option<Result<String, WsError>> {
        loop {
            match self.inner.next().await {
                Some(WsMessage::Text(text)) => return Some(Ok(text)),
                Some(WsMessage::Binary(_)) => continue,
                None => return None,
            }
        }
    }
}

/// Connect to a WebSocket endpoint.
///
/// On WASM, custom headers are not supported by the browser WebSocket API.
/// If `headers` is non-empty, a warning is logged and headers are ignored.
pub async fn connect(
    url: &str,
    headers: &[(&str, &str)],
) -> Result<(WsSender, WsReceiver), WsError> {
    if !headers.is_empty() {
        tracing::warn!(
            "WebSocket headers are not supported on WASM (browser limitation). \
             {} header(s) will be ignored.",
            headers.len()
        );
    }

    let (_meta, ws_stream) = WsMeta::connect(url, None)
        .await
        .map_err(|e| WsError::Connection(e.to_string()))?;

    let (sink, stream) = ws_stream.split();

    Ok((WsSender { inner: sink }, WsReceiver { inner: stream }))
}

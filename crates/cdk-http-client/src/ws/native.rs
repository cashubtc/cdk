//! Native WebSocket implementation using tokio-tungstenite

use futures::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use super::WsError;

/// WebSocket sender half
pub struct WsSender {
    inner: Box<
        dyn futures::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin + Send,
    >,
}

impl std::fmt::Debug for WsSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WsSender").finish_non_exhaustive()
    }
}

/// WebSocket receiver half
pub struct WsReceiver {
    inner: Box<
        dyn futures::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
            + Unpin
            + Send,
    >,
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
            .send(Message::Text(text.into()))
            .await
            .map_err(|e| WsError::Send(e.to_string()))
    }

    /// Send a close frame
    pub async fn close(&mut self) -> Result<(), WsError> {
        self.inner
            .send(Message::Close(None))
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
                Some(Ok(Message::Text(text))) => return Some(Ok(text.to_string())),
                Some(Ok(Message::Close(_))) | None => return None,
                Some(Ok(_)) => continue, // skip binary, ping, pong
                Some(Err(e)) => return Some(Err(WsError::Receive(e.to_string()))),
            }
        }
    }
}

/// Connect to a WebSocket endpoint with optional headers.
///
/// `headers` is a slice of `(name, value)` pairs to include in the upgrade request.
pub async fn connect(
    url: &str,
    headers: &[(&str, &str)],
) -> Result<(WsSender, WsReceiver), WsError> {
    let mut request = url
        .into_client_request()
        .map_err(|e| WsError::Connection(e.to_string()))?;

    for &(name, value) in headers {
        if let (Ok(header_name), Ok(header_value)) = (
            name.parse::<tokio_tungstenite::tungstenite::http::header::HeaderName>(),
            value.parse::<tokio_tungstenite::tungstenite::http::header::HeaderValue>(),
        ) {
            request.headers_mut().insert(header_name, header_value);
        }
    }

    let (ws_stream, _) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|e| WsError::Connection(e.to_string()))?;

    let (sink, stream) = ws_stream.split();

    Ok((
        WsSender {
            inner: Box::new(sink),
        },
        WsReceiver {
            inner: Box::new(stream),
        },
    ))
}

//! Native WebSocket implementation using tokio-tungstenite

use std::pin::Pin;
use std::task::{Context, Poll};

use futures::{Sink, StreamExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

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

// A `Sink<String>` rather than inherent send/close methods so a boxed adapter
// (`stream_channel::from_ws`) forwards `poll_close` to the underlying
// `WebSocketStream`, whose close handshake sends a `Close` frame to the mint.
impl Sink<String> for WsSender {
    type Error = WsError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), WsError>> {
        Pin::new(&mut *self.get_mut().inner)
            .poll_ready(cx)
            .map_err(WsError::from_tungstenite)
    }

    fn start_send(self: Pin<&mut Self>, item: String) -> Result<(), WsError> {
        Pin::new(&mut *self.get_mut().inner)
            .start_send(Message::Text(item.into()))
            .map_err(WsError::from_tungstenite)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), WsError>> {
        Pin::new(&mut *self.get_mut().inner)
            .poll_flush(cx)
            .map_err(WsError::from_tungstenite)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), WsError>> {
        Pin::new(&mut *self.get_mut().inner)
            .poll_close(cx)
            .map_err(WsError::from_tungstenite)
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
                Some(Err(e)) => return Some(Err(WsError::from_tungstenite(e))),
            }
        }
    }
}

/// Adapt an established WebSocket stream to CDK's sender and receiver types.
///
/// This is useful for transports that perform the HTTP upgrade themselves,
/// such as an encrypted tunnel, and then construct a WebSocket stream over the
/// resulting bidirectional byte stream.
pub fn from_websocket_stream<S>(ws_stream: WebSocketStream<S>) -> (WsSender, WsReceiver)
where
    WebSocketStream<S>: futures::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + futures::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin
        + Send
        + 'static,
{
    let (sink, stream) = ws_stream.split();

    (
        WsSender {
            inner: Box::new(sink),
        },
        WsReceiver {
            inner: Box::new(stream),
        },
    )
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
        .map_err(|e| WsError::Terminal(e.to_string()))?;

    for &(name, value) in headers {
        let header_name = name
            .parse::<tokio_tungstenite::tungstenite::http::header::HeaderName>()
            .map_err(|error| {
                WsError::Terminal(format!("invalid WebSocket header name `{name}`: {error}"))
            })?;
        let header_value = value
            .parse::<tokio_tungstenite::tungstenite::http::header::HeaderValue>()
            .map_err(|error| {
                WsError::Terminal(format!(
                    "invalid value for WebSocket header `{name}`: {error}"
                ))
            })?;
        request.headers_mut().insert(header_name, header_value);
    }

    let (ws_stream, _) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(WsError::from_tungstenite)?;

    Ok(from_websocket_stream(ws_stream))
}

/// Connect to a WebSocket endpoint through an Arti Tor client.
#[cfg(feature = "tor")]
pub(crate) async fn connect_tor(
    tor_client: arti_client::TorClient<tor_rtcompat::PreferredRuntime>,
    url: &str,
    headers: &[(&str, &str)],
) -> Result<(WsSender, WsReceiver), WsError> {
    let parsed_url =
        url::Url::parse(url).map_err(|e| WsError::Terminal(format!("Invalid URL: {e}")))?;

    let host = parsed_url
        .host_str()
        .ok_or_else(|| WsError::Terminal("WebSocket URL must include a host".to_string()))?;
    let port = parsed_url
        .port_or_known_default()
        .ok_or_else(|| WsError::Terminal("WebSocket URL must include a port".to_string()))?;

    let mut request = url
        .into_client_request()
        .map_err(|e| WsError::Terminal(e.to_string()))?;

    for &(name, value) in headers {
        let header_name = name
            .parse::<tokio_tungstenite::tungstenite::http::header::HeaderName>()
            .map_err(|error| {
                WsError::Terminal(format!("invalid WebSocket header name `{name}`: {error}"))
            })?;
        let header_value = value
            .parse::<tokio_tungstenite::tungstenite::http::header::HeaderValue>()
            .map_err(|error| {
                WsError::Terminal(format!(
                    "invalid value for WebSocket header `{name}`: {error}"
                ))
            })?;
        request.headers_mut().insert(header_name, header_value);
    }

    let stream = tor_client
        .connect((host, port))
        .await
        .map_err(|e| WsError::Transient(e.to_string()))?;

    let (ws_stream, _) =
        tokio_tungstenite::client_async_tls_with_config(request, stream, None, None)
            .await
            .map_err(WsError::from_tungstenite)?;

    Ok(from_websocket_stream(ws_stream))
}

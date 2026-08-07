//! Native WebSocket implementation using tokio-tungstenite

use std::pin::Pin;
use std::task::{Context, Poll};

use futures::{Sink, StreamExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{Error as HandshakeError, Message};
use tokio_tungstenite::WebSocketStream;

use super::WsError;

/// Map a tungstenite handshake error to a [`WsError`], preserving the "server
/// permanently has no websocket endpoint" case so the caller can latch to poll
/// fallback instead of retrying forever. 429 and 5xx are treated as transient
/// (`Connection`) because they may clear on retry.
fn map_handshake_error(e: HandshakeError) -> WsError {
    match e {
        HandshakeError::Http(resp)
            if matches!(resp.status().as_u16(), 400 | 404 | 405 | 426 | 501) =>
        {
            WsError::Unsupported(resp.status().as_u16())
        }
        other => WsError::Connection(other.to_string()),
    }
}

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
            .map_err(|e| WsError::Send(e.to_string()))
    }

    fn start_send(self: Pin<&mut Self>, item: String) -> Result<(), WsError> {
        Pin::new(&mut *self.get_mut().inner)
            .start_send(Message::Text(item.into()))
            .map_err(|e| WsError::Send(e.to_string()))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), WsError>> {
        Pin::new(&mut *self.get_mut().inner)
            .poll_flush(cx)
            .map_err(|e| WsError::Send(e.to_string()))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), WsError>> {
        Pin::new(&mut *self.get_mut().inner)
            .poll_close(cx)
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
        .map_err(map_handshake_error)?;

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
        url::Url::parse(url).map_err(|e| WsError::Connection(format!("Invalid URL: {e}")))?;

    let host = parsed_url
        .host_str()
        .ok_or_else(|| WsError::Connection("WebSocket URL must include a host".to_string()))?;
    let port = parsed_url
        .port_or_known_default()
        .ok_or_else(|| WsError::Connection("WebSocket URL must include a port".to_string()))?;

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

    let stream = tor_client
        .connect((host, port))
        .await
        .map_err(|e| WsError::Connection(e.to_string()))?;

    let (ws_stream, _) =
        tokio_tungstenite::client_async_tls_with_config(request, stream, None, None)
            .await
            .map_err(map_handshake_error)?;

    Ok(from_websocket_stream(ws_stream))
}

#[cfg(test)]
mod tests {
    use tokio_tungstenite::tungstenite::http::Response;

    use super::*;

    fn http_error(status: u16) -> HandshakeError {
        HandshakeError::Http(Response::builder().status(status).body(None).unwrap())
    }

    #[test]
    fn permanent_statuses_map_to_unsupported() {
        for status in [400, 404, 405, 426, 501] {
            assert!(
                matches!(map_handshake_error(http_error(status)), WsError::Unsupported(s) if s == status),
                "HTTP {status} should latch to Unsupported"
            );
        }
    }

    #[test]
    fn transient_statuses_stay_connection() {
        for status in [429, 500, 502, 503] {
            assert!(
                matches!(
                    map_handshake_error(http_error(status)),
                    WsError::Connection(_)
                ),
                "HTTP {status} should retry as a transient Connection error"
            );
        }
    }

    #[test]
    fn non_http_errors_stay_connection() {
        assert!(matches!(
            map_handshake_error(HandshakeError::ConnectionClosed),
            WsError::Connection(_)
        ));
    }
}

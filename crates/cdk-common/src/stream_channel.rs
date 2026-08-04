//! Transport-neutral bidirectional stream channel.
//!
//! [`StreamTx`] / [`StreamRx`] are the two halves of a duplex channel carrying
//! opaque `String` messages. They are content-agnostic: a caller layers whatever
//! protocol it wants (NUT-17 subscriptions, anything else) on top. Any transport
//! that can produce a `Sink<String>` + `Stream<String>` (a WebSocket for HTTP, a
//! QUIC stream for Iroh, a streaming RPC for gRPC, or an in-memory pair) can
//! vend these halves.

use std::pin::Pin;

use futures::{Sink, SinkExt, Stream, StreamExt};

/// Error carried by a [`StreamTx`] / [`StreamRx`].
#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    /// Sending a message failed.
    #[error("stream send error: {0}")]
    Send(String),
    /// Receiving a message failed.
    #[error("stream receive error: {0}")]
    Receive(String),
}

// The halves are `Send` off wasm (so they cross the mint's and axum's task
// boundaries) and `?Send` on wasm, matching `MintConnector`'s async_trait.
#[cfg(not(target_arch = "wasm32"))]
type BoxedSink = Pin<Box<dyn Sink<String, Error = StreamError> + Send>>;
#[cfg(target_arch = "wasm32")]
type BoxedSink = Pin<Box<dyn Sink<String, Error = StreamError>>>;
#[cfg(not(target_arch = "wasm32"))]
type BoxedStream = Pin<Box<dyn Stream<Item = Result<String, StreamError>> + Send>>;
#[cfg(target_arch = "wasm32")]
type BoxedStream = Pin<Box<dyn Stream<Item = Result<String, StreamError>>>>;

/// The sending half of a [stream channel](self).
pub struct StreamTx {
    inner: BoxedSink,
}

/// The receiving half of a [stream channel](self).
pub struct StreamRx {
    inner: BoxedStream,
}

impl std::fmt::Debug for StreamTx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamTx").finish_non_exhaustive()
    }
}

impl std::fmt::Debug for StreamRx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamRx").finish_non_exhaustive()
    }
}

impl StreamTx {
    /// Wrap a sink of messages.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new<S>(sink: S) -> Self
    where
        S: Sink<String, Error = StreamError> + Send + 'static,
    {
        Self {
            inner: Box::pin(sink),
        }
    }

    /// Wrap a sink of messages.
    #[cfg(target_arch = "wasm32")]
    pub fn new<S>(sink: S) -> Self
    where
        S: Sink<String, Error = StreamError> + 'static,
    {
        Self {
            inner: Box::pin(sink),
        }
    }

    /// Send one message.
    pub async fn send(&mut self, message: String) -> Result<(), StreamError> {
        self.inner.send(message).await
    }

    /// Close the channel.
    pub async fn close(&mut self) -> Result<(), StreamError> {
        self.inner.close().await
    }
}

impl StreamRx {
    /// Wrap a stream of messages.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new<S>(stream: S) -> Self
    where
        S: Stream<Item = Result<String, StreamError>> + Send + 'static,
    {
        Self {
            inner: Box::pin(stream),
        }
    }

    /// Wrap a stream of messages.
    #[cfg(target_arch = "wasm32")]
    pub fn new<S>(stream: S) -> Self
    where
        S: Stream<Item = Result<String, StreamError>> + 'static,
    {
        Self {
            inner: Box::pin(stream),
        }
    }

    /// Receive the next message, or `None` once the channel is closed.
    pub async fn recv(&mut self) -> Option<Result<String, StreamError>> {
        self.inner.next().await
    }
}

/// Wrap an established WebSocket duplex into a transport-neutral stream channel.
/// Used by `MintConnector::open_stream` implementations that dial a WebSocket.
#[cfg(feature = "http")]
pub fn from_ws(
    sender: crate::ws_client::WsSender,
    receiver: crate::ws_client::WsReceiver,
) -> (StreamTx, StreamRx) {
    let tx = StreamTx::new(futures::sink::unfold(
        sender,
        |mut sender, message: String| async move {
            sender
                .send(message)
                .await
                .map_err(|e| StreamError::Send(e.to_string()))?;
            Ok::<_, StreamError>(sender)
        },
    ));
    let rx = StreamRx::new(futures::stream::unfold(
        receiver,
        |mut receiver| async move {
            match receiver.recv().await {
                Some(Ok(message)) => Some((Ok(message), receiver)),
                Some(Err(e)) => Some((Err(StreamError::Receive(e.to_string())), receiver)),
                None => None,
            }
        },
    ));
    (tx, rx)
}

/// Create a connected in-memory duplex: the two returned endpoints are wired to
/// each other, so a message sent on one endpoint's [`StreamTx`] arrives on the
/// other endpoint's [`StreamRx`]. Used to bridge an in-process client and server.
pub fn in_memory_pair() -> ((StreamTx, StreamRx), (StreamTx, StreamRx)) {
    const CAP: usize = 128;
    let (a_out, b_in) = futures::channel::mpsc::channel::<String>(CAP);
    let (b_out, a_in) = futures::channel::mpsc::channel::<String>(CAP);

    let endpoint_a = (
        StreamTx::new(a_out.sink_map_err(|e| StreamError::Send(e.to_string()))),
        StreamRx::new(a_in.map(Ok)),
    );
    let endpoint_b = (
        StreamTx::new(b_out.sink_map_err(|e| StreamError::Send(e.to_string()))),
        StreamRx::new(b_in.map(Ok)),
    );
    (endpoint_a, endpoint_b)
}

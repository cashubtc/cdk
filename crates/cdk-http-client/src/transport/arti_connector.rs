//! Hyper connector for HTTP(S) over Tor via arti.
//!
//! Adapted from the (deprecated) `arti-hyper` 0.19 crate (MIT OR Apache-2.0),
//! with one behavioural fix: arti's [`DataStream`] buffers written bytes into
//! Tor cells and only transmits a partially filled cell when the stream is
//! flushed. TLS implementations driven through `tls-api` write the TLS
//! handshake records and then wait for the server's response without ever
//! flushing the underlying stream, so the ClientHello never leaves the local
//! buffer and the handshake hangs until it times out. [`FlushOnRead`] tracks
//! unflushed writes and drives a flush before the stream is polled for
//! reading, which guarantees handshake bytes reach the wire.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use arti_client::{DataStream, IntoTorAddr, TorClient};
use hyper::client::connect::{Connected, Connection};
use hyper::http::uri::Scheme;
use hyper::http::Uri;
use hyper::service::Service;
use tls_api::TlsConnector as TlsConn;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tor_rtcompat::Runtime;

/// Error making or using an HTTP connection over Tor.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConnectionError {
    /// Unsupported URI scheme
    #[error("unsupported URI scheme in {uri:?}")]
    UnsupportedUriScheme {
        /// URI
        uri: Uri,
    },

    /// Missing hostname
    #[error("Missing hostname in {uri:?}")]
    MissingHostname {
        /// URI
        uri: Uri,
    },

    /// Tor connection failed
    #[error("Tor connection failed")]
    Arti(#[from] arti_client::Error),

    /// TLS connection failed
    #[error("TLS connection failed: {0}")]
    Tls(#[source] Arc<anyhow::Error>),
}

/// Wrapper around [`DataStream`] that flushes pending writes before reads.
///
/// A Tor data stream only sends a partially filled cell on flush; readers that
/// wait for a response to unflushed bytes (like TLS handshakes) would
/// otherwise deadlock.
#[derive(Debug)]
pub struct FlushOnRead {
    /// The underlying Tor stream.
    inner: DataStream,
    /// Whether bytes were written since the last successful flush.
    dirty: bool,
}

impl FlushOnRead {
    /// Wrap a [`DataStream`].
    fn new(inner: DataStream) -> Self {
        Self {
            inner,
            dirty: false,
        }
    }

    /// Flush the underlying stream if any writes are pending.
    fn poll_flush_if_dirty(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        if self.dirty {
            match Pin::new(&mut self.inner).poll_flush(cx) {
                Poll::Ready(Ok(())) => self.dirty = false,
                other => return other,
            }
        }
        Poll::Ready(Ok(()))
    }
}

impl AsyncRead for FlushOnRead {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        let me = self.get_mut();
        match me.poll_flush_if_dirty(cx) {
            Poll::Ready(Ok(())) => {}
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Pending => return Poll::Pending,
        }
        Pin::new(&mut me.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for FlushOnRead {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        let me = self.get_mut();
        let res = Pin::new(&mut me.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = &res {
            if *n > 0 {
                me.dirty = true;
                // Eagerly try to push the partial cell out. TLS layers
                // (`rustls::StreamOwned::flush`, native-tls) never flush the
                // underlying socket, so waiting for an explicit flush call
                // would leave these bytes buffered forever. If this returns
                // Pending or fails, `dirty` stays set and the flush is
                // retried before the next read.
                if let Poll::Ready(Ok(())) = Pin::new(&mut me.inner).poll_flush(cx) {
                    me.dirty = false;
                }
            }
        }
        res
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        let me = self.get_mut();
        let res = Pin::new(&mut me.inner).poll_flush(cx);
        if let Poll::Ready(Ok(())) = &res {
            me.dirty = false;
        }
        res
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        let me = self.get_mut();
        match me.poll_flush_if_dirty(cx) {
            Poll::Ready(Ok(())) => {}
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Pending => return Poll::Pending,
        }
        Pin::new(&mut me.inner).poll_shutdown(cx)
    }
}

/// `hyper` connector that makes HTTP(S) connections via Tor using arti.
pub struct ArtiHttpConnector<R: Runtime, TC: TlsConn> {
    /// The Tor client used to open streams.
    client: TorClient<R>,

    /// TLS used *across* Tor to the origin server (not Tor's own relay TLS).
    tls_conn: Arc<TC>,
}

impl<R: Runtime, TC: TlsConn> Clone for ArtiHttpConnector<R, TC> {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            tls_conn: Arc::clone(&self.tls_conn),
        }
    }
}

impl<R: Runtime, TC: TlsConn> ArtiHttpConnector<R, TC> {
    /// Make a new `ArtiHttpConnector` using an arti `TorClient` object.
    pub fn new(client: TorClient<R>, tls_conn: TC) -> Self {
        Self {
            client,
            tls_conn: Arc::new(tls_conn),
        }
    }
}

/// A Tor-backed `hyper` connection; either bare HTTP or TLS to the origin.
pub struct ArtiHttpConnection<TC: TlsConn> {
    /// The stream
    inner: MaybeHttpsStream<TC>,
}

/// The actual stream; might be TLS, might not.
enum MaybeHttpsStream<TC: TlsConn> {
    /// http
    Http(FlushOnRead),

    /// https
    Https(TC::TlsStream),
}

impl<TC: TlsConn> Connection for ArtiHttpConnection<TC> {
    fn connected(&self) -> Connected {
        Connected::new()
    }
}

impl<TC: TlsConn> AsyncRead for ArtiHttpConnection<TC> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        match &mut self.get_mut().inner {
            MaybeHttpsStream::Http(ds) => Pin::new(ds).poll_read(cx, buf),
            MaybeHttpsStream::Https(t) => Pin::new(t).poll_read(cx, buf),
        }
    }
}

impl<TC: TlsConn> AsyncWrite for ArtiHttpConnection<TC> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        match &mut self.get_mut().inner {
            MaybeHttpsStream::Http(ds) => Pin::new(ds).poll_write(cx, buf),
            MaybeHttpsStream::Https(t) => Pin::new(t).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        match &mut self.get_mut().inner {
            MaybeHttpsStream::Http(ds) => Pin::new(ds).poll_flush(cx),
            MaybeHttpsStream::Https(t) => Pin::new(t).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        match &mut self.get_mut().inner {
            MaybeHttpsStream::Http(ds) => Pin::new(ds).poll_shutdown(cx),
            MaybeHttpsStream::Https(t) => Pin::new(t).poll_shutdown(cx),
        }
    }
}

/// Are we doing TLS?
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum UseTls {
    /// No
    Bare,

    /// Yes
    Tls,
}

/// Convert uri to http\[s\] host and port, and whether to do tls.
fn uri_to_host_port_tls(uri: Uri) -> Result<(String, u16, UseTls), ConnectionError> {
    let use_tls = {
        // Scheme doesn't derive PartialEq so can't be matched on
        let scheme = uri.scheme();
        if scheme == Some(&Scheme::HTTP) {
            UseTls::Bare
        } else if scheme == Some(&Scheme::HTTPS) {
            UseTls::Tls
        } else {
            return Err(ConnectionError::UnsupportedUriScheme { uri });
        }
    };
    let host = match uri.host() {
        Some(h) => h,
        _ => return Err(ConnectionError::MissingHostname { uri }),
    };
    let port = uri.port().map(|x| x.as_u16()).unwrap_or(match use_tls {
        UseTls::Tls => 443,
        UseTls::Bare => 80,
    });

    Ok((host.to_owned(), port, use_tls))
}

impl<R: Runtime, TC: TlsConn> Service<Uri> for ArtiHttpConnector<R, TC> {
    type Response = ArtiHttpConnection<TC>;
    type Error = ConnectionError;
    #[allow(clippy::type_complexity)]
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Uri) -> Self::Future {
        // `TorClient` objects can be cloned cheaply (the cloned objects refer
        // to the same underlying handles required to make Tor connections
        // internally). We use this to avoid the returned future having to
        // borrow `self`.
        let client = self.client.clone();
        let tls_conn = Arc::clone(&self.tls_conn);
        Box::pin(async move {
            // Extract the host and port to connect to from the URI.
            let (host, port, use_tls) = uri_to_host_port_tls(req)?;
            // Initiate a new Tor connection, producing a `DataStream` if
            // successful.
            let addr = (&host as &str, port)
                .into_tor_addr()
                .map_err(arti_client::Error::from)?;
            let ds = FlushOnRead::new(client.connect(addr).await?);

            let inner = match use_tls {
                UseTls::Tls => {
                    let conn = tls_conn
                        .connect_impl_tls_stream(&host, ds)
                        .await
                        .map_err(|e| ConnectionError::Tls(Arc::new(e)))?;
                    MaybeHttpsStream::Https(conn)
                }
                UseTls::Bare => MaybeHttpsStream::Http(ds),
            };

            Ok(ArtiHttpConnection { inner })
        })
    }
}

//! Generic Tower server for HTTP/1.1 transactions carried by Iroh streams.

use std::{
    collections::HashMap,
    error::Error as StdError,
    fmt,
    future::Future,
    io,
    panic::AssertUnwindSafe,
    pin::Pin,
    sync::{atomic::Ordering, Arc, Mutex as StdMutex},
    task::{Context, Poll},
    time::Duration,
};

use axum::body::Body;
use futures::FutureExt;
use http_body_util::Limited;
use hyper::{body::Incoming, header, Request, Response, StatusCode};
use hyper_util::{rt::TokioIo, service::TowerToHyperService};
use iroh::{endpoint::Connection, EndpointId};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::{Notify, OwnedSemaphorePermit, Semaphore},
    task::{JoinHandle, JoinSet},
    time::{Instant, Sleep},
};
use tokio_util::sync::CancellationToken;
use tower::Service;

use crate::{metrics::MetricsInner, Error, IrohNode, IrohStream};

type BoxError = Box<dyn StdError + Send + Sync>;

/// Authenticated peer identity inserted into every bridged request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IrohConnectionInfo {
    /// The endpoint ID authenticated by Iroh's TLS handshake.
    pub remote_endpoint_id: EndpointId,
}

/// Supervised Iroh HTTP server handle.
pub struct IrohServer {
    node: IrohNode,
    cancellation: CancellationToken,
    finished: CancellationToken,
    task: JoinHandle<Result<(), Error>>,
}

impl IrohServer {
    /// Starts serving any cloneable Tower service accepting Axum bodies.
    ///
    /// The same final Axum router used by another listener can be supplied
    /// directly; no route list or Cashu payload mapping is required.
    pub fn start<S, B>(node: IrohNode, service: S) -> Self
    where
        S: Service<Request<Body>, Response = Response<B>> + Clone + Send + 'static,
        S::Future: Send + 'static,
        S::Error: Into<BoxError> + Send,
        B: hyper::body::Body<Data = bytes::Bytes> + Send + 'static,
        B::Data: Send,
        B::Error: Into<BoxError>,
    {
        let cancellation = CancellationToken::new();
        let finished = CancellationToken::new();
        let task_finished = finished.clone();
        let task_node = node.clone();
        let task_cancellation = cancellation.clone();
        let task = tokio::spawn(async move {
            let result = AssertUnwindSafe(accept_loop(task_node, service, task_cancellation))
                .catch_unwind()
                .await
                .unwrap_or(Err(Error::Server));
            task_finished.cancel();
            result
        });
        Self {
            node,
            cancellation,
            finished,
            task,
        }
    }

    /// Returns the server's endpoint ID.
    pub fn endpoint_id(&self) -> EndpointId {
        self.node.endpoint_id()
    }

    /// Resolves when the supervised accept loop exits for any reason.
    pub async fn finished(&self) {
        self.finished.cancelled().await;
    }

    /// Requests graceful drain, closes the endpoint, and waits for all supervised tasks.
    pub async fn shutdown(self) -> Result<(), Error> {
        self.cancellation.cancel();
        self.node.close().await;
        let mut task = self.task;
        match tokio::time::timeout(self.node.config().timeouts.shutdown, &mut task).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(Error::Server),
            Err(_) => {
                task.abort();
                let _ = task.await;
                Err(Error::ShutdownTimeout)
            }
        }
    }
}

impl fmt::Debug for IrohServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IrohServer")
            .field(
                "endpoint_id",
                &self.node.endpoint_id().fmt_short().to_string(),
            )
            .field("shutdown_requested", &self.cancellation.is_cancelled())
            .finish()
    }
}

async fn accept_loop<S, B>(
    node: IrohNode,
    service: S,
    cancellation: CancellationToken,
) -> Result<(), Error>
where
    S: Service<Request<Body>, Response = Response<B>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Into<BoxError> + Send,
    B: hyper::body::Body<Data = bytes::Bytes> + Send + 'static,
    B::Data: Send,
    B::Error: Into<BoxError>,
{
    let limits = node.config().limits;
    let connection_admission = Arc::new(Semaphore::new(limits.max_connections));
    let stream_admission = Arc::new(Semaphore::new(limits.max_streams));
    let peer_admission = Arc::new(PeerAdmission::new(limits.max_connections_per_peer));
    let mut connections = JoinSet::new();

    loop {
        tokio::select! {
            () = cancellation.cancelled() => break,
            result = connections.join_next(), if !connections.is_empty() => {
                if let Some(Err(_)) = result {
                    tracing::warn!("Iroh connection task terminated unexpectedly");
                }
            }
            incoming = node.endpoint().accept() => {
                let Some(incoming) = incoming else {
                    break;
                };
                let Ok(connection_permit) = connection_admission.clone().try_acquire_owned() else {
                    node.metrics_inner().admission_rejections.fetch_add(1, Ordering::Relaxed);
                    drop(incoming);
                    continue;
                };
                let metrics = node.metrics_inner();
                let service = service.clone();
                let expected_authority = node.endpoint_id().to_string();
                let cancellation = cancellation.clone();
                let stream_admission = stream_admission.clone();
                let peer_admission = peer_admission.clone();
                let node_timeouts = node.config().timeouts;
                let handshake_timeout = node_timeouts.connect;
                connections.spawn(async move {
                    let connection = match tokio::time::timeout(handshake_timeout, incoming).await {
                        Ok(Ok(connection)) => connection,
                        Ok(Err(_)) => return,
                        Err(_) => {
                            metrics.timeouts.fetch_add(1, Ordering::Relaxed);
                            return;
                        }
                    };
                    let remote_endpoint_id = connection.remote_id();
                    let Some(peer_guard) = peer_admission.try_admit(remote_endpoint_id) else {
                        metrics.admission_rejections.fetch_add(1, Ordering::Relaxed);
                        connection.close(0_u32.into(), b"peer admission limit");
                        return;
                    };
                    metrics.active_connections.fetch_add(1, Ordering::Relaxed);
                    serve_connection(
                        connection,
                        service,
                        expected_authority,
                        cancellation,
                        stream_admission,
                        node_timeouts.headers,
                        node_timeouts.body_progress,
                        node_timeouts.request,
                        node_timeouts.connection_idle,
                        limits.max_header_bytes,
                        limits.max_request_body_bytes,
                        metrics,
                        connection_permit,
                        peer_guard,
                    )
                    .await;
                });
            }
        }
    }

    while let Some(result) = connections.join_next().await {
        if result.is_err() {
            tracing::warn!("Iroh connection task terminated unexpectedly during shutdown");
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn serve_connection<S, B>(
    connection: Connection,
    service: S,
    expected_authority: String,
    cancellation: CancellationToken,
    global_stream_admission: Arc<Semaphore>,
    header_timeout: Duration,
    body_progress_timeout: Duration,
    request_timeout: Duration,
    connection_idle_timeout: Duration,
    max_header_bytes: usize,
    max_request_body_bytes: usize,
    metrics: Arc<MetricsInner>,
    _connection_permit: OwnedSemaphorePermit,
    _peer_guard: PeerAdmissionGuard,
) where
    S: Service<Request<Body>, Response = Response<B>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Into<BoxError> + Send,
    B: hyper::body::Body<Data = bytes::Bytes> + Send + 'static,
    B::Data: Send,
    B::Error: Into<BoxError>,
{
    let _activity = ConnectionActivity::new(metrics.clone());
    let remote_endpoint_id = connection.remote_id();
    let mut streams = JoinSet::new();
    let idle = tokio::time::sleep(connection_idle_timeout);
    tokio::pin!(idle);
    loop {
        tokio::select! {
            () = cancellation.cancelled() => {
                connection.close(0_u32.into(), b"shutdown");
                break;
            }
            result = streams.join_next(), if !streams.is_empty() => {
                if let Some(Err(_)) = result {
                    tracing::warn!(peer = %remote_endpoint_id.fmt_short(), "Iroh stream task terminated unexpectedly");
                }
                if streams.is_empty() {
                    idle.as_mut().reset(Instant::now() + connection_idle_timeout);
                }
            }
            () = &mut idle, if streams.is_empty() => {
                tracing::debug!(peer = %remote_endpoint_id.fmt_short(), "closing idle Iroh connection");
                connection.close(0_u32.into(), b"idle timeout");
                break;
            }
            stream = connection.accept_bi() => {
                let Ok((send, recv)) = stream else {
                    break;
                };
                let global_permit = tokio::select! {
                    permit = global_stream_admission.clone().acquire_owned() => {
                        let Ok(permit) = permit else {
                            drop(send);
                            drop(recv);
                            break;
                        };
                        permit
                    }
                    () = cancellation.cancelled() => {
                        drop(send);
                        drop(recv);
                        break;
                    }
                };
                streams.spawn(serve_stream(
                    send,
                    recv,
                    service.clone(),
                    IrohConnectionInfo { remote_endpoint_id },
                    expected_authority.clone(),
                    cancellation.clone(),
                    header_timeout,
                    body_progress_timeout,
                    request_timeout,
                    max_header_bytes,
                    max_request_body_bytes,
                    metrics.clone(),
                    global_permit,
                ));
            }
        }
    }
    while let Some(result) = streams.join_next().await {
        if result.is_err() {
            tracing::warn!(peer = %remote_endpoint_id.fmt_short(), "Iroh stream task terminated unexpectedly during shutdown");
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn serve_stream<S, B>(
    send: iroh::endpoint::SendStream,
    recv: iroh::endpoint::RecvStream,
    service: S,
    connection_info: IrohConnectionInfo,
    expected_authority: String,
    cancellation: CancellationToken,
    header_timeout: Duration,
    body_progress_timeout: Duration,
    request_timeout: Duration,
    max_header_bytes: usize,
    max_request_body_bytes: usize,
    metrics: Arc<MetricsInner>,
    global_permit: OwnedSemaphorePermit,
) where
    S: Service<Request<Body>, Response = Response<B>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Into<BoxError> + Send,
    B: hyper::body::Body<Data = bytes::Bytes> + Send + 'static,
    B::Data: Send,
    B::Error: Into<BoxError>,
{
    let stream = TokioIo::new(AdmittedStream::new(
        IrohStream::new(send, recv),
        metrics.clone(),
        global_permit,
    ));
    let request_started = Arc::new(Notify::new());
    let service = ConnectionInfoService {
        inner: service,
        connection_info,
        expected_authority,
        max_request_body_bytes,
        body_progress_timeout,
        request_timeout,
        request_started: request_started.clone(),
        metrics: metrics.clone(),
    };
    let service = TowerToHyperService::new(service);
    let mut builder = hyper::server::conn::http1::Builder::new();
    builder.max_buf_size(max_header_bytes);
    let connection = builder.serve_connection(stream, service).with_upgrades();
    tokio::pin!(connection);
    let headers = tokio::time::sleep(header_timeout);
    tokio::pin!(headers);
    tokio::select! {
        result = &mut connection => {
            if result.is_err() {
                tracing::debug!("Iroh HTTP stream ended with a framing error");
            }
            return;
        }
        () = request_started.notified() => {}
        () = &mut headers => {
            metrics.timeouts.fetch_add(1, Ordering::Relaxed);
            return;
        }
        () = cancellation.cancelled() => return,
    }
    tokio::select! {
        result = &mut connection => {
            if result.is_err() {
                tracing::debug!("Iroh HTTP stream ended with a framing error");
            }
        }
        () = cancellation.cancelled() => {}
    }
}

#[derive(Debug)]
struct AdmittedStream {
    inner: IrohStream,
    _activity: StreamActivity,
    _global_permit: OwnedSemaphorePermit,
}

impl AdmittedStream {
    fn new(
        inner: IrohStream,
        metrics: Arc<MetricsInner>,
        global_permit: OwnedSemaphorePermit,
    ) -> Self {
        Self {
            inner,
            _activity: StreamActivity::new(metrics),
            _global_permit: global_permit,
        }
    }
}

impl AsyncRead for AdmittedStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(context, buffer)
    }
}

impl AsyncWrite for AdmittedStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(context, buffer)
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(context)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(context)
    }
}

#[derive(Debug, Clone)]
struct ConnectionInfoService<S> {
    inner: S,
    connection_info: IrohConnectionInfo,
    expected_authority: String,
    max_request_body_bytes: usize,
    body_progress_timeout: Duration,
    request_timeout: Duration,
    request_started: Arc<Notify>,
    metrics: Arc<MetricsInner>,
}

impl<S, B> Service<Request<Incoming>> for ConnectionInfoService<S>
where
    S: Service<Request<Body>, Response = Response<B>> + Send + 'static,
    S::Future: Send + 'static,
    B: hyper::body::Body<Data = bytes::Bytes> + Send + 'static,
    B::Data: Send,
    B::Error: Into<BoxError>,
{
    type Response = Response<Body>;
    type Error = S::Error;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(&mut self, context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(context)
    }

    fn call(&mut self, request: Request<Incoming>) -> Self::Future {
        self.request_started.notify_one();
        let (mut parts, body) = request.into_parts();
        let authority_matches = parts
            .headers
            .get(header::HOST)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == self.expected_authority);
        if !authority_matches {
            self.metrics.requests.fetch_add(1, Ordering::Relaxed);
            self.metrics
                .record_status(StatusCode::MISDIRECTED_REQUEST.as_u16());
            return Box::pin(async move { Ok(empty_rejection(StatusCode::MISDIRECTED_REQUEST)) });
        }
        if disallowed_hop_by_hop_headers(&parts.headers) {
            self.metrics.requests.fetch_add(1, Ordering::Relaxed);
            self.metrics.record_status(StatusCode::BAD_REQUEST.as_u16());
            return Box::pin(async move { Ok(empty_rejection(StatusCode::BAD_REQUEST)) });
        }
        parts.extensions.insert(self.connection_info);
        let body = Body::new(Limited::new(
            ProgressTimeoutBody::new(body, self.body_progress_timeout, self.metrics.clone()),
            self.max_request_body_bytes,
        ));
        let future = self.inner.call(Request::from_parts(parts, body));
        let request_timeout = self.request_timeout;
        let metrics = self.metrics.clone();
        Box::pin(async move {
            let mut response = match tokio::time::timeout(request_timeout, future).await {
                Ok(response) => response?,
                Err(_) => {
                    metrics.timeouts.fetch_add(1, Ordering::Relaxed);
                    metrics.requests.fetch_add(1, Ordering::Relaxed);
                    metrics.record_status(StatusCode::GATEWAY_TIMEOUT.as_u16());
                    return Ok(empty_rejection(StatusCode::GATEWAY_TIMEOUT));
                }
            };
            metrics.requests.fetch_add(1, Ordering::Relaxed);
            metrics.record_status(response.status().as_u16());
            prepare_response_headers(&mut response);
            Ok(response.map(Body::new))
        })
    }
}

struct ProgressTimeoutBody {
    inner: Incoming,
    timeout: Duration,
    sleep: Pin<Box<Sleep>>,
    metrics: Arc<MetricsInner>,
}

impl ProgressTimeoutBody {
    fn new(inner: Incoming, timeout: Duration, metrics: Arc<MetricsInner>) -> Self {
        Self {
            inner,
            timeout,
            sleep: Box::pin(tokio::time::sleep(timeout)),
            metrics,
        }
    }
}

impl hyper::body::Body for ProgressTimeoutBody {
    type Data = bytes::Bytes;
    type Error = BoxError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        match Pin::new(&mut self.inner).poll_frame(context) {
            Poll::Ready(Some(Ok(frame))) => {
                let deadline = Instant::now() + self.timeout;
                self.sleep.as_mut().reset(deadline);
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(Some(Err(error))) => Poll::Ready(Some(Err(Box::new(error)))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => match self.sleep.as_mut().poll(context) {
                Poll::Ready(()) => {
                    self.metrics.timeouts.fetch_add(1, Ordering::Relaxed);
                    Poll::Ready(Some(Err(Box::new(BodyProgressTimeout))))
                }
                Poll::Pending => Poll::Pending,
            },
        }
    }
}

#[derive(Debug)]
struct BodyProgressTimeout;

impl fmt::Display for BodyProgressTimeout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Iroh request body progress timed out")
    }
}

impl StdError for BodyProgressTimeout {}

fn empty_rejection(status: StatusCode) -> Response<Body> {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONNECTION,
        header::HeaderValue::from_static("close"),
    );
    response
}

fn prepare_response_headers<B>(response: &mut Response<B>) {
    for name in [
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
    ] {
        response.headers_mut().remove(name);
    }
    let websocket_upgrade = response.status() == StatusCode::SWITCHING_PROTOCOLS
        && response
            .headers()
            .get(header::UPGRADE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.eq_ignore_ascii_case("websocket"));
    if !websocket_upgrade {
        response.headers_mut().remove(header::UPGRADE);
        response.headers_mut().insert(
            header::CONNECTION,
            header::HeaderValue::from_static("close"),
        );
    }
}

fn disallowed_hop_by_hop_headers(headers: &hyper::HeaderMap) -> bool {
    const FORBIDDEN: [&str; 6] = [
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
    ];
    if FORBIDDEN.iter().any(|name| headers.contains_key(*name)) {
        return true;
    }

    let connection_upgrade = headers
        .get(header::CONNECTION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
        });
    let websocket_upgrade = headers
        .get(header::UPGRADE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket"));
    match (
        headers.contains_key(header::CONNECTION),
        headers.contains_key(header::UPGRADE),
    ) {
        (false, false) => false,
        (true, true) => !(connection_upgrade && websocket_upgrade),
        (true, false) | (false, true) => true,
    }
}

#[derive(Debug)]
struct PeerAdmission {
    maximum: usize,
    counts: StdMutex<HashMap<EndpointId, usize>>,
}

impl PeerAdmission {
    fn new(maximum: usize) -> Self {
        Self {
            maximum,
            counts: StdMutex::new(HashMap::new()),
        }
    }

    fn try_admit(self: &Arc<Self>, peer: EndpointId) -> Option<PeerAdmissionGuard> {
        let mut counts = self
            .counts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let count = counts.entry(peer).or_default();
        if *count >= self.maximum {
            return None;
        }
        *count += 1;
        Some(PeerAdmissionGuard {
            admission: self.clone(),
            peer,
        })
    }
}

#[derive(Debug)]
struct PeerAdmissionGuard {
    admission: Arc<PeerAdmission>,
    peer: EndpointId,
}

impl Drop for PeerAdmissionGuard {
    fn drop(&mut self) {
        let mut counts = self
            .admission
            .counts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let remove = match counts.get_mut(&self.peer) {
            Some(count) => {
                *count = count.saturating_sub(1);
                *count == 0
            }
            None => false,
        };
        if remove {
            counts.remove(&self.peer);
        }
    }
}

#[derive(Debug)]
struct ConnectionActivity(Arc<MetricsInner>);

impl ConnectionActivity {
    fn new(metrics: Arc<MetricsInner>) -> Self {
        Self(metrics)
    }
}

impl Drop for ConnectionActivity {
    fn drop(&mut self) {
        self.0.active_connections.fetch_sub(1, Ordering::Relaxed);
    }
}

#[derive(Debug)]
struct StreamActivity(Arc<MetricsInner>);

impl StreamActivity {
    fn new(metrics: Arc<MetricsInner>) -> Self {
        metrics.active_streams.fetch_add(1, Ordering::Relaxed);
        Self(metrics)
    }
}

impl Drop for StreamActivity {
    fn drop(&mut self) {
        self.0.active_streams.fetch_sub(1, Ordering::Relaxed);
    }
}

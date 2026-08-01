//! End-to-end feasibility tests for generic HTTP/1.1 and WebSocket bridging.

use std::{
    error::Error,
    net::{Ipv4Addr, SocketAddrV4},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use axum::{
    body::{Body, Bytes},
    extract::{
        ws::{Message as AxumMessage, WebSocket},
        State, WebSocketUpgrade,
    },
    http::{header::HOST, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use cdk_iroh::{protocol, IrohStream};
use futures::{future::join_all, SinkExt, StreamExt};
use http_body_util::{BodyExt, Full};
use hyper::{Method, Request as HyperRequest, StatusCode as HyperStatusCode};
use hyper_util::{rt::TokioIo, service::TowerToHyperService};
use iroh::{
    endpoint::{presets, Connection},
    Endpoint, EndpointAddr,
};
use tokio::{
    task::{JoinHandle, JoinSet},
    time::timeout,
};
use tokio_tungstenite::{
    client_async,
    tungstenite::{client::IntoClientRequest, Message},
};
use tokio_util::sync::CancellationToken;

type TestError = Box<dyn Error + Send + Sync>;
type TestResult<T = ()> = Result<T, TestError>;

#[derive(Debug, Clone)]
struct AppState {
    authority: Arc<str>,
    websocket_tasks: Arc<AtomicUsize>,
}

#[derive(Debug)]
struct ActivityGuard(Arc<AtomicUsize>);

impl ActivityGuard {
    fn new(counter: Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self(counter)
    }
}

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Debug)]
struct HttpResult {
    status: HyperStatusCode,
    body: Bytes,
}

#[derive(Debug)]
struct SpikeServer {
    endpoint: Endpoint,
    authority: Arc<str>,
    cancellation: CancellationToken,
    accept_task: JoinHandle<TestResult>,
    accepted_connections: Arc<AtomicUsize>,
    active_stream_tasks: Arc<AtomicUsize>,
    websocket_tasks: Arc<AtomicUsize>,
}

impl SpikeServer {
    async fn start() -> TestResult<Self> {
        let endpoint = Endpoint::builder(presets::Minimal)
            .bind_addr(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))?
            .alpns(vec![protocol::ALPN.to_vec()])
            .bind()
            .await?;
        let authority: Arc<str> = endpoint.id().to_string().into();
        let websocket_tasks = Arc::new(AtomicUsize::new(0));
        let state = AppState {
            authority: authority.clone(),
            websocket_tasks: websocket_tasks.clone(),
        };
        let router = Router::new()
            .route("/future-route", get(|| async { "future-route" }))
            .route("/echo", post(|body: Bytes| async move { body }))
            .route(
                "/status-error",
                get(|| async { (StatusCode::IM_A_TEAPOT, "teapot") }),
            )
            .route("/ws", get(websocket_upgrade))
            .layer(middleware::from_fn_with_state(
                state.clone(),
                validate_authority,
            ))
            .with_state(state);

        let cancellation = CancellationToken::new();
        let accepted_connections = Arc::new(AtomicUsize::new(0));
        let active_stream_tasks = Arc::new(AtomicUsize::new(0));
        let accept_task = tokio::spawn(accept_loop(
            endpoint.clone(),
            router,
            cancellation.clone(),
            accepted_connections.clone(),
            active_stream_tasks.clone(),
        ));

        Ok(Self {
            endpoint,
            authority,
            cancellation,
            accept_task,
            accepted_connections,
            active_stream_tasks,
            websocket_tasks,
        })
    }

    fn endpoint_addr(&self) -> EndpointAddr {
        self.endpoint.addr()
    }

    async fn shutdown(self) -> TestResult {
        self.cancellation.cancel();
        self.endpoint.close().await;
        timeout(protocol::SHUTDOWN_TIMEOUT, self.accept_task).await???;
        assert_eq!(self.active_stream_tasks.load(Ordering::SeqCst), 0);
        assert_eq!(self.websocket_tasks.load(Ordering::SeqCst), 0);
        Ok(())
    }
}

async fn validate_authority(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let matches = request
        .headers()
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == state.authority.as_ref());
    if !matches {
        return StatusCode::MISDIRECTED_REQUEST.into_response();
    }
    next.run(request).await
}

async fn websocket_upgrade(
    State(state): State<AppState>,
    upgrade: WebSocketUpgrade,
) -> impl IntoResponse {
    upgrade.on_upgrade(move |socket| websocket_echo(socket, state.websocket_tasks))
}

async fn websocket_echo(mut socket: WebSocket, counter: Arc<AtomicUsize>) {
    let _guard = ActivityGuard::new(counter);
    while let Some(Ok(message)) = socket.recv().await {
        match message {
            AxumMessage::Text(_) | AxumMessage::Binary(_) => {
                if socket.send(message).await.is_err() {
                    break;
                }
            }
            AxumMessage::Close(_) => break,
            AxumMessage::Ping(_) | AxumMessage::Pong(_) => {}
        }
    }
}

async fn accept_loop(
    endpoint: Endpoint,
    router: Router,
    cancellation: CancellationToken,
    accepted_connections: Arc<AtomicUsize>,
    active_stream_tasks: Arc<AtomicUsize>,
) -> TestResult {
    let mut connection_tasks = JoinSet::new();
    loop {
        tokio::select! {
            () = cancellation.cancelled() => break,
            incoming = endpoint.accept() => {
                let Some(incoming) = incoming else {
                    break;
                };
                let connection = incoming.await?;
                accepted_connections.fetch_add(1, Ordering::SeqCst);
                connection_tasks.spawn(serve_connection(
                    connection,
                    router.clone(),
                    cancellation.clone(),
                    active_stream_tasks.clone(),
                ));
            }
        }
    }
    while let Some(result) = connection_tasks.join_next().await {
        result??;
    }
    Ok(())
}

async fn serve_connection(
    connection: Connection,
    router: Router,
    cancellation: CancellationToken,
    active_stream_tasks: Arc<AtomicUsize>,
) -> TestResult {
    let mut stream_tasks = JoinSet::new();
    loop {
        tokio::select! {
            () = cancellation.cancelled() => {
                connection.close(0_u32.into(), b"shutdown");
                break;
            }
            stream = connection.accept_bi() => {
                let Ok((send, recv)) = stream else {
                    break;
                };
                let router = router.clone();
                let cancellation = cancellation.clone();
                let counter = active_stream_tasks.clone();
                stream_tasks.spawn(async move {
                    let _guard = ActivityGuard::new(counter);
                    let stream = TokioIo::new(IrohStream::new(send, recv));
                    let service = TowerToHyperService::new(router);
                    let connection = hyper::server::conn::http1::Builder::new()
                        .serve_connection(stream, service)
                        .with_upgrades();
                    tokio::select! {
                        result = connection => {
                            // A malformed or disconnected client terminates only its
                            // stream; it must not tear down the shared Iroh connection.
                            let _ = result;
                            Ok::<(), TestError>(())
                        },
                        () = cancellation.cancelled() => Ok(()),
                    }
                });
            }
        }
    }
    while let Some(result) = stream_tasks.join_next().await {
        match result? {
            Ok(()) => {}
            Err(error) if cancellation.is_cancelled() => {
                let _ = error;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

async fn local_endpoint() -> TestResult<Endpoint> {
    Ok(Endpoint::builder(presets::Minimal)
        .bind_addr(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))?
        .bind()
        .await?)
}

async fn request(
    connection: Connection,
    authority: Arc<str>,
    method: Method,
    path: &'static str,
    body: Bytes,
) -> TestResult<HttpResult> {
    let (send, recv) = timeout(protocol::STREAM_OPEN_TIMEOUT, connection.open_bi()).await??;
    let stream = TokioIo::new(IrohStream::new(send, recv));
    let (mut sender, driver) = hyper::client::conn::http1::handshake(stream).await?;
    let driver = tokio::spawn(driver);
    let request = HyperRequest::builder()
        .method(method)
        .uri(path)
        .header(HOST, authority.as_ref())
        .body(Full::new(body))?;
    let response = timeout(protocol::HEADER_TIMEOUT, sender.send_request(request)).await??;
    drop(sender);
    let status = response.status();
    let body = timeout(
        protocol::BODY_PROGRESS_TIMEOUT,
        response.into_body().collect(),
    )
    .await??
    .to_bytes();
    timeout(protocol::SHUTDOWN_TIMEOUT, driver).await???;
    Ok(HttpResult { status, body })
}

async fn wait_for_zero(counter: &AtomicUsize) -> TestResult {
    timeout(Duration::from_secs(5), async {
        while counter.load(Ordering::SeqCst) != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    Ok(())
}

#[test]
fn protocol_constants_are_golden() {
    assert_eq!(protocol::ALPN, b"cashu-cdk-http/1");
    assert_eq!(protocol::HTTP_VERSION, "HTTP/1.1");
    assert_eq!(protocol::MAX_HEADER_BYTES, 65_536);
    assert_eq!(protocol::MAX_REQUEST_BODY_BYTES, 1_048_576);
    assert_eq!(protocol::MAX_RESPONSE_BODY_BYTES, 16_777_216);
    assert_eq!(protocol::CONNECT_TIMEOUT, Duration::from_secs(15));
    assert_eq!(protocol::STREAM_OPEN_TIMEOUT, Duration::from_secs(10));
    assert_eq!(protocol::HEADER_TIMEOUT, Duration::from_secs(15));
    assert_eq!(protocol::BODY_PROGRESS_TIMEOUT, Duration::from_secs(30));
    assert_eq!(protocol::SHUTDOWN_TIMEOUT, Duration::from_secs(10));
}

#[tokio::test(flavor = "multi_thread")]
async fn generic_http_and_websocket_share_one_iroh_connection() -> TestResult {
    let server = SpikeServer::start().await?;
    let client = local_endpoint().await?;
    let connection = timeout(
        protocol::CONNECT_TIMEOUT,
        client.connect(server.endpoint_addr(), protocol::ALPN),
    )
    .await??;

    let ws_stream = {
        let (send, recv) = connection.open_bi().await?;
        IrohStream::new(send, recv)
    };
    let ws_request = format!("ws://{}/ws", server.authority).into_client_request()?;
    let (mut websocket, response) = client_async(ws_request, ws_stream).await?;
    assert_eq!(response.status(), HyperStatusCode::SWITCHING_PROTOCOLS);
    let first_message = r#"{"jsonrpc":"2.0","method":"subscribe","id":1}"#;
    websocket.send(Message::Text(first_message.into())).await?;
    assert_eq!(
        websocket.next().await.transpose()?,
        Some(Message::Text(first_message.into()))
    );

    let get_result = request(
        connection.clone(),
        server.authority.clone(),
        Method::GET,
        "/future-route?new=1",
        Bytes::new(),
    )
    .await?;
    assert_eq!(get_result.status, HyperStatusCode::OK);
    assert_eq!(get_result.body, "future-route");

    let post_result = request(
        connection.clone(),
        server.authority.clone(),
        Method::POST,
        "/echo",
        Bytes::from_static(b"ordinary-cashu-json-bytes"),
    )
    .await?;
    assert_eq!(post_result.status, HyperStatusCode::OK);
    assert_eq!(post_result.body, "ordinary-cashu-json-bytes");

    let error_result = request(
        connection.clone(),
        server.authority.clone(),
        Method::GET,
        "/status-error",
        Bytes::new(),
    )
    .await?;
    assert_eq!(error_result.status, HyperStatusCode::IM_A_TEAPOT);
    assert_eq!(error_result.body, "teapot");

    let rejected = request(
        connection.clone(),
        "another-endpoint".into(),
        Method::GET,
        "/future-route",
        Bytes::new(),
    )
    .await?;
    assert_eq!(rejected.status, HyperStatusCode::MISDIRECTED_REQUEST);

    let concurrent = (0..16).map(|_| {
        request(
            connection.clone(),
            server.authority.clone(),
            Method::GET,
            "/future-route",
            Bytes::new(),
        )
    });
    for result in join_all(concurrent).await {
        let result = result?;
        assert_eq!(result.status, HyperStatusCode::OK);
        assert_eq!(result.body, "future-route");
    }

    let second_message = r#"{"jsonrpc":"2.0","method":"ping","id":2}"#;
    websocket.send(Message::Text(second_message.into())).await?;
    assert_eq!(
        websocket.next().await.transpose()?,
        Some(Message::Text(second_message.into()))
    );
    websocket.close(None).await?;

    assert_eq!(server.accepted_connections.load(Ordering::SeqCst), 1);
    connection.close(0_u32.into(), b"test complete");
    client.close().await;
    server.shutdown().await
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_either_endpoint_cancels_partial_streams_without_task_leaks() -> TestResult {
    let server = SpikeServer::start().await?;
    let client = local_endpoint().await?;
    let connection = client
        .connect(server.endpoint_addr(), protocol::ALPN)
        .await?;
    let (mut send, _recv) = connection.open_bi().await?;
    send.write_all(b"GET /").await?;
    timeout(Duration::from_secs(5), async {
        while server.active_stream_tasks.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    client.close().await;
    wait_for_zero(&server.active_stream_tasks).await?;
    server.shutdown().await?;

    let server = SpikeServer::start().await?;
    let client = local_endpoint().await?;
    let connection = client
        .connect(server.endpoint_addr(), protocol::ALPN)
        .await?;
    let (mut send, _recv) = connection.open_bi().await?;
    send.write_all(b"POST /echo HTTP/1.1\r\n").await?;
    timeout(Duration::from_secs(5), async {
        while server.active_stream_tasks.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    server.shutdown().await?;
    client.close().await;
    Ok(())
}

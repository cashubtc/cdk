//! Standalone transport conformance against a synthetic Axum router.

use std::{
    convert::Infallible,
    error::Error as StdError,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use async_trait::async_trait;
use axum::{
    body::{Body, Bytes},
    extract::{
        ws::{Message, WebSocket},
        Extension, Form, Json, WebSocketUpgrade,
    },
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use cashu::nuts::nut22::AuthToken;
use cdk_http_client::{HttpError, RawResponse, Transport};
use cdk_iroh::{
    protocol, DiscoveryMode, Error, IrohClient, IrohConfig, IrohConnectionInfo, IrohLimits,
    IrohNode, IrohServer, IrohStream, IrohTimeouts, IrohTransport, RelayUrl, SecretKey,
};
use futures::{future::join_all, stream};
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::{body::Frame, header, Method};
use hyper_util::rt::TokioIo;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;
use url::Url;

type TestError = Box<dyn StdError + Send + Sync>;
type TestResult<T = ()> = Result<T, TestError>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Echo {
    value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FormPayload {
    name: String,
    count: u32,
}

async fn json_echo(Json(payload): Json<Echo>) -> Json<Echo> {
    Json(payload)
}

async fn form_echo(Form(payload): Form<FormPayload>) -> Json<FormPayload> {
    Json(payload)
}

async fn raw_query(uri: Uri) -> String {
    uri.query().unwrap_or_default().to_owned()
}

async fn auth_echo(headers: HeaderMap) -> Response {
    match headers
        .get("clear-auth")
        .and_then(|value| value.to_str().ok())
    {
        Some("transport-secret") => Json(json!({ "authenticated": true })).into_response(),
        _ => StatusCode::UNAUTHORIZED.into_response(),
    }
}

async fn connection_info(Extension(info): Extension<IrohConnectionInfo>) -> String {
    info.remote_endpoint_id.to_string()
}

async fn stream_response() -> Body {
    let chunks = ["abcd", "efgh", "ijkl"]
        .into_iter()
        .map(|chunk| Ok::<Bytes, Infallible>(Bytes::from_static(chunk.as_bytes())));
    Body::from_stream(stream::iter(chunks))
}

async fn websocket(upgrade: WebSocketUpgrade) -> impl IntoResponse {
    upgrade.on_upgrade(websocket_echo)
}

async fn websocket_echo(mut socket: WebSocket) {
    while let Some(Ok(message)) = socket.recv().await {
        match message {
            Message::Text(_) | Message::Binary(_) => {
                if socket.send(message).await.is_err() {
                    break;
                }
            }
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) => {}
        }
    }
}

fn synthetic_router() -> Router {
    Router::new()
        .route("/raw", get(raw_query))
        .route("/json", post(json_echo))
        .route("/form", post(form_echo))
        .route("/auth", get(auth_echo))
        .route(
            "/status-error",
            get(|| async { (StatusCode::IM_A_TEAPOT, "teapot") }),
        )
        .route("/stream", post(stream_response))
        .route("/peer", get(connection_info))
        .route("/future-route", get(|| async { "future-route" }))
        .route("/echo-bytes", post(|body: Bytes| async move { body }))
        .route("/ws", get(websocket))
}

fn loopback_config() -> IrohConfig {
    IrohConfig::static_only()
        .with_bind_addr(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
}

fn iroh_url(endpoint_id: cdk_iroh::EndpointId, path: &str) -> TestResult<Url> {
    Ok(Url::parse(&format!("iroh://{endpoint_id}{path}"))?)
}

async fn wait_for(mut predicate: impl FnMut() -> bool) -> TestResult {
    tokio::time::timeout(Duration::from_secs(5), async move {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    Ok(())
}

struct Fixture {
    server_node: IrohNode,
    client_node: IrohNode,
    server: IrohServer,
    transport: IrohTransport<MockHttp>,
}

impl Fixture {
    async fn start() -> TestResult<Self> {
        let server_node = IrohNode::ephemeral(loopback_config()).await?;
        let client_node =
            IrohNode::ephemeral(loopback_config().with_ticket(server_node.endpoint_ticket()))
                .await?;
        let server = IrohServer::start(server_node.clone(), synthetic_router());
        let transport = IrohTransport::with_node(client_node.clone(), MockHttp::default());
        Ok(Self {
            server_node,
            client_node,
            server,
            transport,
        })
    }

    fn url(&self, path: &str) -> TestResult<Url> {
        iroh_url(self.server_node.endpoint_id(), path)
    }

    async fn shutdown(self) -> TestResult {
        self.server.shutdown().await?;
        self.client_node.close().await;
        assert_eq!(self.server_node.metrics().active_connections, 0);
        assert_eq!(self.server_node.metrics().active_streams, 0);
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
struct MockHttp {
    calls: Arc<AtomicUsize>,
    proxy_calls: Arc<AtomicUsize>,
    dns_calls: Arc<AtomicUsize>,
}

impl MockHttp {
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Transport for MockHttp {
    fn with_proxy(
        &mut self,
        _proxy: Url,
        _host_matcher: Option<&str>,
        _accept_invalid_certs: bool,
    ) -> Result<(), HttpError> {
        self.proxy_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn resolve_dns_txt(&self, _domain: &str) -> Result<Vec<String>, HttpError> {
        self.dns_calls.fetch_add(1, Ordering::SeqCst);
        Ok(vec!["delegated-dns".to_string()])
    }

    async fn http_get<R>(&self, _url: Url, _auth: Option<AuthToken>) -> Result<R, HttpError>
    where
        R: DeserializeOwned,
    {
        self.calls.fetch_add(1, Ordering::SeqCst);
        serde_json::from_value(json!({ "delegated": true })).map_err(HttpError::from)
    }

    async fn http_get_raw(
        &self,
        _url: Url,
        _auth: Option<AuthToken>,
    ) -> Result<RawResponse, HttpError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(RawResponse::new(200, b"delegated".to_vec()))
    }

    async fn http_post<P, R>(
        &self,
        _url: Url,
        _auth_token: Option<AuthToken>,
        _payload: &P,
    ) -> Result<R, HttpError>
    where
        P: Serialize + Send + Sync,
        R: DeserializeOwned,
    {
        self.calls.fetch_add(1, Ordering::SeqCst);
        serde_json::from_value(json!({ "delegated": true })).map_err(HttpError::from)
    }

    async fn http_post_form_raw<P>(
        &self,
        _url: Url,
        _auth_token: Option<AuthToken>,
        _payload: &P,
    ) -> Result<RawResponse, HttpError>
    where
        P: Serialize + Send + Sync,
    {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(RawResponse::new(200, b"delegated".to_vec()))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn endpoint_identities_and_discovery_modes_are_explicit() -> TestResult {
    let first = IrohNode::ephemeral(loopback_config()).await?;
    let second = IrohNode::ephemeral(loopback_config()).await?;
    assert_ne!(first.endpoint_id(), second.endpoint_id());
    first.close().await;
    second.close().await;
    assert!(!first.metrics().endpoint_online);
    assert!(!second.metrics().endpoint_online);

    let key = SecretKey::generate();
    let persistent_first = IrohNode::persistent(loopback_config(), key.clone()).await?;
    let expected_id = persistent_first.endpoint_id();
    persistent_first.close().await;
    let persistent_second = IrohNode::persistent(loopback_config(), key).await?;
    assert_eq!(persistent_second.endpoint_id(), expected_id);
    persistent_second.close().await;

    let n0 = IrohNode::ephemeral(IrohConfig {
        discovery: DiscoveryMode::N0,
        bind_addr: loopback_config().bind_addr,
        ..IrohConfig::default()
    })
    .await?;
    n0.close().await;

    let relay: RelayUrl = "https://127.0.0.1:9".parse()?;
    let custom = IrohNode::ephemeral(IrohConfig {
        discovery: DiscoveryMode::Custom {
            relay_urls: vec![relay],
        },
        bind_addr: loopback_config().bind_addr,
        ..IrohConfig::default()
    })
    .await?;
    custom.close().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn transport_conformance_and_pooling() -> TestResult {
    let fixture = Fixture::start().await?;

    let raw = fixture
        .transport
        .http_get_raw(fixture.url("/raw?alpha=1&beta=two")?, None)
        .await?;
    assert_eq!(raw.status(), 200);
    assert_eq!(raw.text().await?, "alpha=1&beta=two");

    let payload = Echo {
        value: "ordinary-json".to_string(),
    };
    let echoed: Echo = fixture
        .transport
        .http_post(fixture.url("/json")?, None, &payload)
        .await?;
    assert_eq!(echoed, payload);

    let form = FormPayload {
        name: "operator".to_string(),
        count: 5,
    };
    let form_response = fixture
        .transport
        .http_post_form_raw(fixture.url("/form")?, None, &form)
        .await?;
    assert_eq!(form_response.json::<FormPayload>().await?, form);

    let auth: Value = fixture
        .transport
        .http_get(
            fixture.url("/auth")?,
            Some(AuthToken::ClearAuth("transport-secret".to_string())),
        )
        .await?;
    assert_eq!(auth, json!({ "authenticated": true }));

    let status = fixture
        .transport
        .http_get_raw(fixture.url("/status-error")?, None)
        .await?;
    assert_eq!(status.status(), StatusCode::IM_A_TEAPOT.as_u16());
    assert_eq!(status.text().await?, "teapot");
    let status_error = fixture
        .transport
        .http_get::<Value>(fixture.url("/status-error")?, None)
        .await
        .expect_err("non-success JSON request must preserve status error");
    assert!(matches!(
        status_error,
        HttpError::Status { status: 418, .. }
    ));

    let peer = fixture
        .transport
        .http_get_raw(fixture.url("/peer")?, None)
        .await?
        .text()
        .await?;
    assert_eq!(peer, fixture.client_node.endpoint_id().to_string());

    let future = fixture
        .transport
        .http_get_raw(fixture.url("/future-route")?, None)
        .await?;
    assert_eq!(future.text().await?, "future-route");

    let client = IrohClient::new(fixture.client_node.clone());
    let stream = client
        .request_raw(
            Method::POST,
            fixture.url("/stream")?,
            &[(
                header::CONTENT_TYPE,
                header::HeaderValue::from_static("application/json"),
            )],
            Bytes::from_static(b"{}"),
            12,
        )
        .await?;
    assert_eq!(stream.bytes().await?, b"abcdefghijkl");
    let bounded = client
        .request_raw(Method::POST, fixture.url("/stream")?, &[], Bytes::new(), 8)
        .await
        .expect_err("streaming body must be rejected while reading");
    assert!(matches!(
        bounded,
        Error::ResponseTooLarge { actual: 12, max: 8 }
            | Error::ResponseTooLarge {
                actual: 12..,
                max: 8
            }
    ));

    let hop_by_hop = client
        .request_raw(
            Method::GET,
            fixture.url("/future-route")?,
            &[(
                header::CONNECTION,
                header::HeaderValue::from_static("close"),
            )],
            Bytes::new(),
            64,
        )
        .await?;
    assert_eq!(hop_by_hop.status(), StatusCode::BAD_REQUEST.as_u16());

    let concurrent = (0..24).map(|_| {
        fixture
            .transport
            .http_get_raw(fixture.url("/future-route").expect("valid Iroh URL"), None)
    });
    for response in join_all(concurrent).await {
        assert_eq!(response?.status(), 200);
    }

    let websocket_url = fixture.url("/ws")?.to_string();
    let (mut sender, mut receiver) = fixture.transport.ws_connect(&websocket_url, &[]).await?;
    sender.send("websocket-over-iroh".to_string()).await?;
    assert_eq!(
        receiver.recv().await.transpose()?,
        Some("websocket-over-iroh".to_string())
    );
    sender.close().await?;

    let metrics = fixture.client_node.metrics();
    assert_eq!(metrics.connection_attempts, 1);
    assert_eq!(metrics.connection_failures, 0);
    assert!(metrics.pool_hits >= 30);
    assert!(fixture.server_node.metrics().requests >= 32);

    fixture.shutdown().await
}

#[tokio::test(flavor = "multi_thread")]
async fn request_and_response_limits_are_enforced_before_unbounded_allocation() -> TestResult {
    let server_node = IrohNode::ephemeral(IrohConfig {
        limits: IrohLimits {
            max_request_body_bytes: 4,
            ..IrohLimits::default()
        },
        ..loopback_config()
    })
    .await?;
    let client_node =
        IrohNode::ephemeral(loopback_config().with_ticket(server_node.endpoint_ticket())).await?;
    let server = IrohServer::start(server_node.clone(), synthetic_router());
    let client = IrohClient::new(client_node.clone());
    let response = client
        .request_raw(
            Method::POST,
            iroh_url(server_node.endpoint_id(), "/echo-bytes")?,
            &[],
            Bytes::from_static(b"12345"),
            64,
        )
        .await?;
    assert!(!response.is_success());

    let tiny_client = IrohNode::ephemeral(IrohConfig {
        limits: IrohLimits {
            max_request_body_bytes: 4,
            ..IrohLimits::default()
        },
        ..loopback_config().with_ticket(server_node.endpoint_ticket())
    })
    .await?;
    let error = IrohClient::new(tiny_client.clone())
        .request_raw(
            Method::POST,
            iroh_url(server_node.endpoint_id(), "/echo-bytes")?,
            &[],
            Bytes::from_static(b"12345"),
            64,
        )
        .await
        .expect_err("oversized request must be rejected before dialing");
    assert!(matches!(
        error,
        Error::RequestTooLarge { actual: 5, max: 4 }
    ));
    let framing_override = IrohClient::new(tiny_client.clone())
        .request_raw(
            Method::GET,
            iroh_url(server_node.endpoint_id(), "/future-route")?,
            &[(
                header::HOST,
                header::HeaderValue::from_static("transport-override.invalid"),
            )],
            Bytes::new(),
            64,
        )
        .await
        .expect_err("callers must not override the authenticated authority");
    assert!(matches!(framing_override, Error::InvalidRequest { .. }));
    assert_eq!(tiny_client.metrics().connection_attempts, 0);

    tiny_client.close().await;
    server.shutdown().await?;
    client_node.close().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn authenticated_endpoint_and_http_authority_must_agree() -> TestResult {
    let fixture = Fixture::start().await?;
    let raw_endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .bind_addr(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))?
        .bind()
        .await?;
    let connection = raw_endpoint
        .connect(fixture.server_node.endpoint_addr(), protocol::ALPN)
        .await?;
    let (send, recv) = connection.open_bi().await?;
    let stream = TokioIo::new(IrohStream::new(send, recv));
    let (mut sender, driver) = hyper::client::conn::http1::handshake(stream).await?;
    let driver = tokio::spawn(driver);
    let request = hyper::Request::builder()
        .method(Method::GET)
        .uri("/future-route")
        .header(header::HOST, SecretKey::generate().public().to_string())
        .body(Full::new(Bytes::new()))?;
    let response = sender.send_request(request).await?;
    assert_eq!(response.status(), StatusCode::MISDIRECTED_REQUEST);
    assert!(response.into_body().collect().await?.to_bytes().is_empty());
    drop(sender);
    driver.await??;
    raw_endpoint.close().await;
    fixture.shutdown().await
}

#[tokio::test(flavor = "multi_thread")]
async fn stream_admission_rejects_excess_work_and_recovers_capacity() -> TestResult {
    let server_node = IrohNode::ephemeral(IrohConfig {
        limits: IrohLimits {
            max_streams: 1,
            max_streams_per_connection: 1,
            ..IrohLimits::default()
        },
        ..loopback_config()
    })
    .await?;
    let client_node =
        IrohNode::ephemeral(loopback_config().with_ticket(server_node.endpoint_ticket())).await?;
    let server = IrohServer::start(server_node.clone(), synthetic_router());
    let transport = IrohTransport::with_node(client_node.clone(), MockHttp::default());
    let websocket_url = iroh_url(server_node.endpoint_id(), "/ws")?.to_string();
    let (mut sender, receiver) = transport.ws_connect(&websocket_url, &[]).await?;
    wait_for(|| server_node.metrics().active_streams == 1).await?;

    let rejected = tokio::time::timeout(
        Duration::from_secs(2),
        transport.http_get_raw(iroh_url(server_node.endpoint_id(), "/future-route")?, None),
    )
    .await?
    .expect_err("second stream must be rejected while the WebSocket holds capacity");
    assert!(matches!(
        rejected,
        HttpError::Connection(_) | HttpError::Timeout
    ));
    assert!(server_node.metrics().admission_rejections >= 1);

    sender.close().await?;
    drop(sender);
    drop(receiver);
    wait_for(|| server_node.metrics().active_streams == 0).await?;
    let recovered = transport
        .http_get_raw(iroh_url(server_node.endpoint_id(), "/future-route")?, None)
        .await?;
    assert_eq!(recovered.status(), 200);

    server.shutdown().await?;
    client_node.close().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn stalled_request_headers_and_bodies_time_out_and_release_capacity() -> TestResult {
    let server_node = IrohNode::ephemeral(IrohConfig {
        timeouts: IrohTimeouts {
            headers: Duration::from_millis(200),
            body_progress: Duration::from_millis(200),
            ..IrohTimeouts::default()
        },
        limits: IrohLimits {
            max_streams: 1,
            max_streams_per_connection: 1,
            ..IrohLimits::default()
        },
        ..loopback_config()
    })
    .await?;
    let server = IrohServer::start(server_node.clone(), synthetic_router());
    let raw_endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .bind_addr(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))?
        .bind()
        .await?;
    let connection = raw_endpoint
        .connect(server_node.endpoint_addr(), protocol::ALPN)
        .await?;

    let (mut stalled_send, stalled_recv) = connection.open_bi().await?;
    stalled_send.write_all(b"G").await?;
    stalled_send.flush().await?;
    wait_for(|| server_node.metrics().active_streams == 1).await?;
    wait_for(|| server_node.metrics().active_streams == 0).await?;
    assert!(server_node.metrics().timeouts >= 1);
    drop(stalled_send);
    drop(stalled_recv);

    let (send, recv) = connection.open_bi().await?;
    let stream = TokioIo::new(IrohStream::new(send, recv));
    let (mut sender, driver) = hyper::client::conn::http1::handshake(stream).await?;
    let driver = tokio::spawn(driver);
    let pending_body = stream::pending::<Result<Frame<Bytes>, Infallible>>();
    let request = hyper::Request::builder()
        .method(Method::POST)
        .uri("/echo-bytes")
        .header(header::HOST, server_node.endpoint_id().to_string())
        .header(header::CONTENT_LENGTH, "1")
        .body(StreamBody::new(pending_body))?;
    let response =
        tokio::time::timeout(Duration::from_secs(2), sender.send_request(request)).await?;
    if let Ok(response) = response {
        assert!(!response.status().is_success());
        response.into_body().collect().await?;
    }
    drop(sender);
    let _ = tokio::time::timeout(Duration::from_secs(2), driver).await?;
    wait_for(|| server_node.metrics().timeouts >= 2).await?;

    raw_endpoint.close().await;
    server.shutdown().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn connection_admission_rejects_and_recovers_global_capacity() -> TestResult {
    let server_node = IrohNode::ephemeral(IrohConfig {
        limits: IrohLimits {
            max_connections: 1,
            ..IrohLimits::default()
        },
        ..loopback_config()
    })
    .await?;
    let client_config = IrohConfig {
        timeouts: IrohTimeouts {
            connect: Duration::from_secs(2),
            ..IrohTimeouts::default()
        },
        ..loopback_config().with_ticket(server_node.endpoint_ticket())
    };
    let first_client = IrohNode::ephemeral(client_config.clone()).await?;
    let second_client = IrohNode::ephemeral(client_config).await?;
    let server = IrohServer::start(server_node.clone(), synthetic_router());
    let first = IrohTransport::with_node(first_client.clone(), MockHttp::default());
    let second = IrohTransport::with_node(second_client.clone(), MockHttp::default());

    assert_eq!(
        first
            .http_get_raw(iroh_url(server_node.endpoint_id(), "/future-route")?, None)
            .await?
            .status(),
        200
    );
    wait_for(|| server_node.metrics().active_connections == 1).await?;
    let rejected = second
        .http_get_raw(iroh_url(server_node.endpoint_id(), "/future-route")?, None)
        .await
        .expect_err("a second connection must be rejected at global capacity");
    assert!(matches!(
        rejected,
        HttpError::Connection(_) | HttpError::Timeout
    ));
    assert!(server_node.metrics().admission_rejections >= 1);

    first_client.close().await;
    wait_for(|| server_node.metrics().active_connections == 0).await?;
    assert_eq!(
        second
            .http_get_raw(iroh_url(server_node.endpoint_id(), "/future-route")?, None)
            .await?
            .status(),
        200
    );

    server.shutdown().await?;
    second_client.close().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn per_peer_connection_admission_releases_after_disconnect() -> TestResult {
    let server_node = IrohNode::ephemeral(IrohConfig {
        limits: IrohLimits {
            max_connections: 4,
            max_connections_per_peer: 1,
            ..IrohLimits::default()
        },
        ..loopback_config()
    })
    .await?;
    let server = IrohServer::start(server_node.clone(), synthetic_router());
    let raw_endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .bind_addr(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))?
        .bind()
        .await?;
    let first = raw_endpoint
        .connect(server_node.endpoint_addr(), protocol::ALPN)
        .await?;
    wait_for(|| server_node.metrics().active_connections == 1).await?;
    let second = raw_endpoint
        .connect(server_node.endpoint_addr(), protocol::ALPN)
        .await;
    wait_for(|| server_node.metrics().admission_rejections >= 1).await?;
    if let Ok(second) = second {
        second.close(0_u32.into(), b"test cleanup");
    }
    assert_eq!(server_node.metrics().active_connections, 1);

    first.close(0_u32.into(), b"test release");
    wait_for(|| server_node.metrics().active_connections == 0).await?;
    let third = raw_endpoint
        .connect(server_node.endpoint_addr(), protocol::ALPN)
        .await?;
    wait_for(|| server_node.metrics().active_connections == 1).await?;
    third.close(0_u32.into(), b"test cleanup");
    raw_endpoint.close().await;
    server.shutdown().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn outgoing_pool_is_bounded_and_reclaims_closed_entries() -> TestResult {
    let first_server_node = IrohNode::ephemeral(loopback_config()).await?;
    let second_server_node = IrohNode::ephemeral(loopback_config()).await?;
    let client_node = IrohNode::ephemeral(IrohConfig {
        limits: IrohLimits {
            max_pooled_connections: 1,
            ..IrohLimits::default()
        },
        static_tickets: vec![
            first_server_node.endpoint_ticket(),
            second_server_node.endpoint_ticket(),
        ],
        ..loopback_config()
    })
    .await?;
    let first_server = IrohServer::start(first_server_node.clone(), synthetic_router());
    let second_server = IrohServer::start(second_server_node.clone(), synthetic_router());
    let client = IrohClient::new(client_node.clone());
    assert_eq!(
        client
            .get_raw(
                iroh_url(first_server_node.endpoint_id(), "/future-route")?,
                None
            )
            .await?
            .status(),
        200
    );
    let saturated = client
        .get_raw(
            iroh_url(second_server_node.endpoint_id(), "/future-route")?,
            None,
        )
        .await
        .expect_err("a second live pool destination must exceed the configured bound");
    assert!(matches!(saturated, Error::Admission { .. }));

    first_server.shutdown().await?;
    let second_url = iroh_url(second_server_node.endpoint_id(), "/future-route")?;
    let recovered = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match client.get_raw(second_url.clone(), None).await {
                Ok(response) => break Ok::<RawResponse, Error>(response),
                Err(Error::Admission { .. }) => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(error) => break Err(error),
            }
        }
    })
    .await??;
    assert_eq!(recovered.status(), 200);

    second_server.shutdown().await?;
    client_node.close().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn scheme_dispatch_delegates_exactly_and_never_falls_back() -> TestResult {
    let node = IrohNode::ephemeral(IrohConfig {
        timeouts: IrohTimeouts {
            connect: Duration::from_millis(250),
            ..IrohTimeouts::default()
        },
        ..loopback_config()
    })
    .await?;
    let mock = MockHttp::default();
    let mut transport = IrohTransport::with_node(node.clone(), mock.clone());

    let delegated: Value = transport
        .http_get(Url::parse("https://example.invalid/test")?, None)
        .await?;
    assert_eq!(delegated, json!({ "delegated": true }));
    assert_eq!(mock.calls(), 1);

    transport.with_proxy(
        Url::parse("http://proxy.example.invalid")?,
        Some("example.invalid"),
        false,
    )?;
    assert_eq!(mock.proxy_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        transport.resolve_dns_txt("example.invalid").await?,
        vec!["delegated-dns"]
    );

    let unsupported = transport
        .http_get_raw(Url::parse("ftp://example.invalid/file")?, None)
        .await
        .expect_err("unsupported schemes must fail explicitly");
    assert!(unsupported.to_string().contains("unsupported URL scheme"));
    assert_eq!(mock.calls(), 1);

    let unknown = SecretKey::generate().public();
    let full_unknown = unknown.to_string();
    let connect_error = transport
        .http_get_raw(iroh_url(unknown, "/never-fallback")?, None)
        .await
        .expect_err("unknown Iroh endpoint must fail without HTTPS fallback");
    assert!(matches!(
        &connect_error,
        HttpError::Connection(_) | HttpError::Timeout
    ));
    assert!(!connect_error.to_string().contains(&full_unknown));
    assert_eq!(mock.calls(), 1);

    node.close().await;
    Ok(())
}

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::str::FromStr;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use async_trait::async_trait;
use axum::body::Body;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use cdk_common::{nut19, Amount, AuthToken, CurrencyUnit, MeltQuoteRequest, MintQuoteRequest};
use cdk_http_client::{Async, HttpError, RawResponse, Transport};
use cdk_iroh::{IrohConfig, IrohNode, IrohServer, IrohTransport};
use futures::StreamExt;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use url::Url;

use super::http_client::HttpClient;
use super::MintConnector;
use crate::mint_url::MintUrl;
use crate::nuts::nut00::{KnownMethod, PaymentMethod};
use crate::nuts::nut04::{MintQuoteCustomRequest, MintRequest};
use crate::nuts::nut05::{MeltQuoteCustomRequest, MeltRequest};
use crate::nuts::nut07::CheckStateRequest;
use crate::nuts::nut09::RestoreRequest;
use crate::nuts::nut22::MintAuthRequest;
use crate::nuts::nut23::{MeltQuoteBolt11Request, MintQuoteBolt11Request};
use crate::nuts::nut25::{MeltQuoteBolt12Request, MintQuoteBolt12Request};
use crate::nuts::nut29::{BatchCheckMintQuoteRequest, BatchMintRequest};
use crate::nuts::nut30::{MeltQuoteOnchainRequest, MintQuoteOnchainRequest};
use crate::nuts::{Id, PublicKey, SwapRequest};
use crate::wallet::subscription::websocket_url;
use crate::Bolt11Invoice;

const KEYSET_ID: &str = "00ffd48b8f5ecf80";
const PUBLIC_KEY: &str = "02194603ffa062682c4f10e2dfe8f53e17d5d0329db51c8d3935cc74a4c0e0d4cb";
const BOLT11: &str = "lnbc100n1p5z3a63pp56854ytysg7e5z9fl3w5mgvrlqjfcytnjv8ff5hm5qt6gl6alxesqdqqcqzzsxqyz5vqsp5p0x0dlhn27s63j4emxnk26p7f94u0lyarnfp5yqmac9gzy4ngdss9qxpqysgqne3v0hnzt2lp0hc69xpzckk0cdcar7glvjhq60lsrfe8gejdm8c564prrnsft6ctxxyrewp4jtezrq3gxxqnfjj0f9tw2qs9y0lslmqpfu7et9";

#[derive(Debug, Clone, Default)]
struct FixtureHttpsTransport {
    target: Option<Url>,
    inner: Async,
}

impl FixtureHttpsTransport {
    fn new(target: Url) -> Self {
        Self {
            target: Some(target),
            inner: Async::default(),
        }
    }

    fn rewrite(&self, url: &Url) -> Result<Url, HttpError> {
        let mut rewritten = self
            .target
            .clone()
            .ok_or_else(|| HttpError::Build("fixture target is not configured".to_string()))?;
        rewritten.set_path(url.path());
        rewritten.set_query(url.query());
        Ok(rewritten)
    }

    fn rewrite_websocket(&self, url: &str) -> Result<Url, cdk_http_client::ws::WsError> {
        let original = Url::parse(url).map_err(|_| {
            cdk_http_client::ws::WsError::Connection("fixture WebSocket URL is invalid".to_string())
        })?;
        let mut rewritten = self.target.clone().ok_or_else(|| {
            cdk_http_client::ws::WsError::Connection("fixture target is not configured".to_string())
        })?;
        rewritten.set_scheme("ws").map_err(|_| {
            cdk_http_client::ws::WsError::Connection(
                "fixture WebSocket scheme is invalid".to_string(),
            )
        })?;
        rewritten.set_path(original.path());
        rewritten.set_query(original.query());
        Ok(rewritten)
    }
}

#[async_trait]
impl Transport for FixtureHttpsTransport {
    async fn ws_connect(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<
        (
            cdk_http_client::ws::WsSender,
            cdk_http_client::ws::WsReceiver,
        ),
        cdk_http_client::ws::WsError,
    > {
        self.inner
            .ws_connect(self.rewrite_websocket(url)?.as_str(), headers)
            .await
    }

    fn with_proxy(
        &mut self,
        _proxy: Url,
        _host_matcher: Option<&str>,
        _accept_invalid_certs: bool,
    ) -> Result<(), HttpError> {
        Ok(())
    }

    async fn resolve_dns_txt(&self, domain: &str) -> Result<Vec<String>, HttpError> {
        Ok(vec![format!("fixture={domain}")])
    }

    async fn http_get<R>(&self, url: Url, auth: Option<AuthToken>) -> Result<R, HttpError>
    where
        R: DeserializeOwned,
    {
        self.inner.http_get(self.rewrite(&url)?, auth).await
    }

    async fn http_get_raw(
        &self,
        url: Url,
        auth: Option<AuthToken>,
    ) -> Result<RawResponse, HttpError> {
        self.inner.http_get_raw(self.rewrite(&url)?, auth).await
    }

    async fn http_post<P, R>(
        &self,
        url: Url,
        auth_token: Option<AuthToken>,
        payload: &P,
    ) -> Result<R, HttpError>
    where
        P: Serialize + Send + Sync,
        R: DeserializeOwned,
    {
        self.inner
            .http_post(self.rewrite(&url)?, auth_token, payload)
            .await
    }

    async fn http_post_form_raw<P>(
        &self,
        url: Url,
        auth_token: Option<AuthToken>,
        payload: &P,
    ) -> Result<RawResponse, HttpError>
    where
        P: Serialize + Send + Sync,
    {
        self.inner
            .http_post_form_raw(self.rewrite(&url)?, auth_token, payload)
            .await
    }
}

#[derive(Clone, Default)]
struct FixtureState {
    requests: Arc<Mutex<Vec<String>>>,
    swap_attempts: Arc<AtomicUsize>,
    authenticated_requests: Arc<AtomicUsize>,
}

impl FixtureState {
    fn reset_retry(&self) {
        self.swap_attempts.store(0, Ordering::SeqCst);
    }

    fn request_count(&self) -> usize {
        self.requests.lock().expect("fixture request lock").len()
    }
}

async fn fixture_websocket(ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(|mut socket| async move {
        while let Some(Ok(message)) = socket.next().await {
            if socket.send(message).await.is_err() {
                break;
            }
        }
    })
}

async fn fixture_api(State(state): State<FixtureState>, request: Request<Body>) -> Response {
    let path = request.uri().path().to_owned();
    state
        .requests
        .lock()
        .expect("fixture request lock")
        .push(path.clone());

    if path == "/v1/auth/blind/mint"
        && (request.headers().contains_key("clear-auth")
            || request.headers().contains_key("blind-auth"))
    {
        state.authenticated_requests.fetch_add(1, Ordering::SeqCst);
    }

    if path == "/v1/swap" && state.swap_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
        return (StatusCode::SERVICE_UNAVAILABLE, "retry once").into_response();
    }

    let parts = path.trim_start_matches('/').split('/').collect::<Vec<_>>();
    let response = match parts.as_slice() {
        ["v1", "keys"] | ["v1", "keys", _] => keys_response(),
        ["v1", "auth", "blind", "keys", _] => keys_response(),
        ["v1", "keysets"] | ["v1", "auth", "blind", "keysets"] => {
            json!({ "keysets": [] })
        }
        ["v1", "info"] => mint_info_response(),
        ["v1", "mint", "quote", method, tail @ ..] => {
            let quote = mint_quote_response(method);
            if tail.last() == Some(&"check") {
                json!([quote])
            } else {
                quote
            }
        }
        ["v1", "mint", _, ..] | ["v1", "auth", "blind", "mint"] => {
            json!({ "signatures": [] })
        }
        ["v1", "melt", "quote", method, ..] | ["v1", "melt", method] => melt_quote_response(method),
        ["v1", "swap"] => json!({ "signatures": [] }),
        ["v1", "checkstate"] => json!({ "states": [] }),
        ["v1", "restore"] => json!({ "outputs": [], "signatures": [] }),
        ["lnurl", "pay"] => json!({
            "callback": "https://fixture.invalid/lnurl/invoice",
            "minSendable": 1,
            "maxSendable": 1_000_000,
            "metadata": "[[\"text/plain\",\"fixture\"]]",
            "tag": "payRequest",
            "reason": null
        }),
        ["lnurl", "invoice"] => json!({
            "pr": BOLT11,
            "success_action": null,
            "routes": null,
            "reason": null
        }),
        ["future", "automatic"] => json!({ "future": true }),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };

    Json(response).into_response()
}

fn keys_response() -> Value {
    json!({
        "keysets": [{
            "id": KEYSET_ID,
            "unit": "sat",
            "active": true,
            "keys": { "1": PUBLIC_KEY },
            "input_fee_ppk": 0
        }]
    })
}

fn mint_info_response() -> Value {
    let info = crate::nuts::MintInfo {
        name: Some("transport fixture".to_string()),
        nuts: crate::nuts::nut06::Nuts {
            nut19: nut19::Settings {
                ttl: Some(1),
                cached_endpoints: vec![nut19::CachedEndpoint::new(
                    nut19::Method::Post,
                    nut19::Path::Swap,
                )],
            },
            ..Default::default()
        },
        ..Default::default()
    };
    serde_json::to_value(info).expect("serialize fixture mint info")
}

fn mint_quote_response(method: &str) -> Value {
    match method {
        "bolt11" => json!({
            "quote": "quote",
            "request": BOLT11,
            "amount": 1,
            "unit": "sat",
            "amount_paid": 1,
            "amount_issued": 0,
            "state": "PAID",
            "expiry": 4_000_000_000_u64
        }),
        "bolt12" => json!({
            "quote": "quote",
            "request": "lno1fixture",
            "amount": 1,
            "unit": "sat",
            "expiry": 4_000_000_000_u64,
            "pubkey": PUBLIC_KEY,
            "amount_paid": 1,
            "amount_issued": 0
        }),
        "onchain" => json!({
            "quote": "quote",
            "request": "bcrt1qfixture",
            "unit": "sat",
            "expiry": 4_000_000_000_u64,
            "pubkey": PUBLIC_KEY,
            "amount_paid": 1,
            "amount_issued": 0
        }),
        custom => json!({
            "quote": "quote",
            "request": "custom:fixture",
            "method": custom,
            "amount": 1,
            "amount_paid": 1,
            "amount_issued": 0,
            "unit": "sat",
            "expiry": 4_000_000_000_u64
        }),
    }
}

fn melt_quote_response(method: &str) -> Value {
    match method {
        "onchain" => json!({
            "quote": "quote",
            "amount": 1,
            "unit": "sat",
            "state": "UNPAID",
            "expiry": 4_000_000_000_u64,
            "request": "bcrt1qfixture",
            "fee_options": [{
                "fee_index": 0,
                "fee_reserve": 0,
                "estimated_blocks": 1
            }],
            "selected_fee_index": 0,
            "outpoint": null,
            "change": null
        }),
        "paypal" => json!({
            "quote": "quote",
            "method": "paypal",
            "amount": 1,
            "fee_reserve": 0,
            "state": "UNPAID",
            "expiry": 4_000_000_000_u64,
            "request": "custom:fixture",
            "unit": "sat",
            "payment_preimage": null,
            "change": null
        }),
        _ => json!({
            "quote": "quote",
            "amount": 1,
            "fee_reserve": 0,
            "state": "UNPAID",
            "expiry": 4_000_000_000_u64,
            "request": BOLT11,
            "unit": "sat",
            "payment_preimage": null,
            "change": null
        }),
    }
}

async fn run_connector_conformance<C>(client: &C, mint_url: MintUrl, state: &FixtureState)
where
    C: MintConnector + Send + Sync,
{
    state.reset_retry();
    let requests_before = state.request_count();
    let public_key = PublicKey::from_str(PUBLIC_KEY).expect("fixture public key");
    let keyset_id = Id::from_str(KEYSET_ID).expect("fixture keyset id");

    client.get_mint_keys().await.expect("get mint keys");
    client
        .get_mint_keyset(keyset_id)
        .await
        .expect("get mint keyset");
    client.get_mint_keysets().await.expect("get mint keysets");

    let mint_quote_requests = [
        MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
            amount: Amount::from(1),
            unit: CurrencyUnit::Sat,
            description: None,
            pubkey: None,
        }),
        MintQuoteRequest::Bolt12(MintQuoteBolt12Request {
            amount: Some(Amount::from(1)),
            unit: CurrencyUnit::Sat,
            description: None,
            pubkey: public_key,
        }),
        MintQuoteRequest::Onchain(MintQuoteOnchainRequest {
            unit: CurrencyUnit::Sat,
            pubkey: public_key,
        }),
        MintQuoteRequest::Custom {
            method: PaymentMethod::Custom("paypal".to_string()),
            request: MintQuoteCustomRequest {
                amount: Some(Amount::from(1)),
                unit: CurrencyUnit::Sat,
                description: None,
                pubkey: None,
                extra: Value::Null,
            },
        },
    ];
    for request in mint_quote_requests {
        client
            .post_mint_quote(request)
            .await
            .expect("post mint quote variant");
    }

    let methods = [
        PaymentMethod::Known(KnownMethod::Bolt11),
        PaymentMethod::Known(KnownMethod::Bolt12),
        PaymentMethod::Known(KnownMethod::Onchain),
        PaymentMethod::Custom("paypal".to_string()),
    ];
    for method in &methods {
        client
            .get_mint_quote_status(method.clone(), "quote")
            .await
            .expect("get mint quote variant");
        client
            .post_mint(
                method,
                MintRequest {
                    quote: "quote".to_string(),
                    outputs: Vec::new(),
                    signature: None,
                },
            )
            .await
            .expect("post mint variant");
        client
            .post_batch_check_mint_quote_status(
                method,
                BatchCheckMintQuoteRequest {
                    quotes: vec!["quote".to_string()],
                },
            )
            .await
            .expect("batch check mint quote variant");
        client
            .post_batch_mint(
                method,
                BatchMintRequest {
                    quotes: vec!["quote".to_string()],
                    quote_amounts: None,
                    outputs: Vec::new(),
                    signatures: None,
                },
            )
            .await
            .expect("batch mint variant");
    }

    let invoice = Bolt11Invoice::from_str(BOLT11).expect("fixture invoice");
    let melt_quote_requests = [
        MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
            request: invoice,
            unit: CurrencyUnit::Sat,
            options: None,
        }),
        MeltQuoteRequest::Bolt12(MeltQuoteBolt12Request {
            request: "lno1fixture".to_string(),
            unit: CurrencyUnit::Sat,
            options: None,
        }),
        MeltQuoteRequest::Onchain(MeltQuoteOnchainRequest {
            request: "bcrt1qfixture".to_string(),
            unit: CurrencyUnit::Sat,
            amount: Amount::from(1),
        }),
        MeltQuoteRequest::Custom(MeltQuoteCustomRequest {
            method: "paypal".to_string(),
            request: "custom:fixture".to_string(),
            unit: CurrencyUnit::Sat,
            amount: Some(Amount::from(1)),
            extra: Value::Null,
        }),
    ];
    for request in melt_quote_requests {
        client
            .post_melt_quote(request)
            .await
            .expect("post melt quote variant");
    }

    for method in &methods {
        client
            .get_melt_quote_status(method.clone(), "quote")
            .await
            .expect("get melt quote variant");
        let request = match method {
            PaymentMethod::Known(KnownMethod::Onchain) => {
                MeltRequest::new("quote".to_string(), Vec::new(), None).fee_index(0)
            }
            _ => MeltRequest::new("quote".to_string(), Vec::new(), None),
        };
        client
            .post_melt(method, request)
            .await
            .expect("post melt variant");
    }

    client.get_mint_info().await.expect("get mint info");
    client
        .post_swap(SwapRequest::new(Vec::new(), Vec::new()))
        .await
        .expect("NUT-19 retried swap");
    assert_eq!(state.swap_attempts.load(Ordering::SeqCst), 2);
    client
        .post_check_state(CheckStateRequest { ys: Vec::new() })
        .await
        .expect("check state");
    client
        .post_restore(RestoreRequest {
            outputs: Vec::new(),
        })
        .await
        .expect("restore");

    client.get_auth_wallet().await;
    client.set_auth_wallet(None).await;
    let auth_requests_before = state.authenticated_requests.load(Ordering::SeqCst);
    let auth = client.auth_connector(
        mint_url.clone(),
        Some(AuthToken::ClearAuth("fixture-cat".to_string())),
    );
    auth.get_auth_token().await.expect("get auth token");
    auth.set_auth_token(AuthToken::ClearAuth("fixture-cat-2".to_string()))
        .await
        .expect("set auth token");
    auth.get_mint_info().await.expect("auth mint info");
    auth.get_mint_blind_auth_keyset(keyset_id)
        .await
        .expect("auth keyset");
    auth.get_mint_blind_auth_keysets()
        .await
        .expect("auth keysets");
    auth.post_mint_blind_auth(MintAuthRequest {
        outputs: Vec::new(),
    })
    .await
    .expect("auth mint");
    assert_eq!(
        state.authenticated_requests.load(Ordering::SeqCst),
        auth_requests_before + 1
    );

    let _oidc = client.oidc_client(
        "https://fixture.invalid/oidc".to_string(),
        Some("fixture-client".to_string()),
    );
    client
        .fetch_lnurl_pay_request("https://fixture.invalid/lnurl/pay")
        .await
        .expect("LNURL pay request");
    client
        .fetch_lnurl_invoice("https://fixture.invalid/lnurl/invoice")
        .await
        .expect("LNURL invoice");
    #[cfg(feature = "bip353")]
    assert_eq!(
        client
            .resolve_dns_txt("fixture.invalid")
            .await
            .expect("DNS delegation"),
        vec!["fixture=fixture.invalid".to_string()]
    );

    let ws_url = websocket_url(&mint_url).expect("supported NUT-17 URL");
    let (mut sender, mut receiver) = client
        .connect_websocket(ws_url.as_str(), &[("x-fixture", "present")])
        .await
        .expect("NUT-17 WebSocket");
    sender
        .send("fixture-subscription".to_string())
        .await
        .expect("send subscription frame");
    assert_eq!(
        receiver
            .recv()
            .await
            .expect("subscription response")
            .expect("valid subscription frame"),
        "fixture-subscription"
    );
    sender.close().await.expect("close subscription");

    assert!(state.request_count() > requests_before);
}

#[tokio::test]
async fn hybrid_wallet_connector_conformance_over_https_and_iroh() {
    let state = FixtureState::default();
    let router = Router::new()
        .route("/v1/ws", get(fixture_websocket))
        .fallback(fixture_api)
        .with_state(state.clone());

    let listener = TcpListener::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
        .await
        .expect("bind fixture HTTP listener");
    let http_addr = listener.local_addr().expect("fixture HTTP address");
    let (http_shutdown_tx, http_shutdown_rx) = oneshot::channel::<()>();
    let http_task = tokio::spawn({
        let router = router.clone();
        async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = http_shutdown_rx.await;
                })
                .await
                .expect("serve fixture HTTP router");
        }
    });

    let bind_addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
    let server_node = IrohNode::ephemeral(IrohConfig::static_only().with_bind_addr(bind_addr))
        .await
        .expect("create fixture Iroh server node");
    let server_ticket = server_node.endpoint_ticket();
    let server = IrohServer::start(server_node, router);
    let client_node = IrohNode::ephemeral(
        IrohConfig::static_only()
            .with_bind_addr(bind_addr)
            .with_ticket(server_ticket),
    )
    .await
    .expect("create fixture Iroh client node");

    let fixture_target = Url::parse(&format!("http://{http_addr}")).expect("fixture target URL");
    let hybrid = IrohTransport::with_node(
        client_node.clone(),
        FixtureHttpsTransport::new(fixture_target),
    );
    let https_url = MintUrl::from_str("https://fixture.invalid").expect("HTTPS mint URL");
    let iroh_url =
        MintUrl::from_str(&format!("iroh://{}", server.endpoint_id())).expect("Iroh mint URL");
    let https_client = HttpClient::with_transport(https_url.clone(), hybrid.clone(), None);
    let iroh_client = HttpClient::with_transport(iroh_url.clone(), hybrid.clone(), None);

    run_connector_conformance(&https_client, https_url, &state).await;
    run_connector_conformance(&iroh_client, iroh_url.clone(), &state).await;

    let default_alias: super::HttpClient = super::HttpClient::with_transport(
        iroh_url,
        IrohTransport::with_node(client_node.clone(), Async::default()),
        None,
    );
    default_alias
        .get_mint_info()
        .await
        .expect("feature-selected default alias uses Iroh");

    let future: Value = hybrid
        .http_get(
            Url::parse(&format!("iroh://{}/future/automatic", server.endpoint_id()))
                .expect("future route URL"),
            None,
        )
        .await
        .expect("future router-only route over Iroh");
    assert_eq!(future, json!({ "future": true }));

    let dead_node = IrohNode::ephemeral(IrohConfig::static_only().with_bind_addr(bind_addr))
        .await
        .expect("create unavailable Iroh node");
    let dead_id = dead_node.endpoint_id();
    client_node.add_ticket(dead_node.endpoint_ticket());
    dead_node.close().await;
    let requests_before = state.request_count();
    let unavailable = HttpClient::with_transport(
        MintUrl::from_str(&format!("iroh://{dead_id}")).expect("unavailable Iroh mint URL"),
        hybrid,
        None,
    );
    assert!(unavailable.get_mint_info().await.is_err());
    assert_eq!(
        state.request_count(),
        requests_before,
        "Iroh connection failure must not attempt HTTPS fallback"
    );

    client_node.close().await;
    server.shutdown().await.expect("shutdown Iroh server");
    let _ = http_shutdown_tx.send(());
    http_task.await.expect("join fixture HTTP server");
}

#[test]
fn nut17_scheme_mapping_preserves_iroh_and_rejects_unknown_schemes() {
    let https = MintUrl::from_str("https://fixture.invalid").expect("HTTPS URL");
    assert_eq!(websocket_url(&https).expect("HTTPS WS").scheme(), "wss");
    let http = MintUrl::from_str("http://fixture.invalid").expect("HTTP URL");
    assert_eq!(websocket_url(&http).expect("HTTP WS").scheme(), "ws");
    let iroh = MintUrl::from_str(&format!(
        "iroh://{}",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    ))
    .expect("Iroh URL");
    assert_eq!(websocket_url(&iroh).expect("Iroh WS").scheme(), "iroh");
    let ftp = MintUrl::from_str("ftp://fixture.invalid").expect("FTP URL");
    assert!(websocket_url(&ftp).is_err());
}

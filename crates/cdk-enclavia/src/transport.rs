use core::fmt;

use async_trait::async_trait;
use cdk::mint_url::MintUrl;
use cdk::nuts::AuthToken;
use cdk_http_client::{Async, HttpError, RawResponse, Transport};
use enclavia::{Client, Method};
use serde::de::DeserializeOwned;
use serde::Serialize;
use url::{Origin, Url};

#[cfg(not(target_arch = "wasm32"))]
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
#[cfg(not(target_arch = "wasm32"))]
use tokio_tungstenite::tungstenite::http::header::{HeaderName, HeaderValue, HOST};
#[cfg(not(target_arch = "wasm32"))]
use tokio_tungstenite::tungstenite::protocol::Role;
#[cfg(not(target_arch = "wasm32"))]
use tokio_tungstenite::WebSocketStream;

use crate::error::{Error, Result};

#[cfg(not(target_arch = "wasm32"))]
fn websocket_upgrade_headers(
    url: &str,
    headers: &[(&str, &str)],
) -> std::result::Result<Vec<(String, String)>, cdk_http_client::ws::WsError> {
    let mut request = url.into_client_request().map_err(|error| {
        cdk_http_client::ws::WsError::Connection(format!(
            "could not construct WebSocket upgrade request: {error}"
        ))
    })?;

    for &(name, value) in headers {
        let header_name = name.parse::<HeaderName>().map_err(|error| {
            cdk_http_client::ws::WsError::Connection(format!(
                "invalid WebSocket header name `{name}`: {error}"
            ))
        })?;
        let header_value = value.parse::<HeaderValue>().map_err(|error| {
            cdk_http_client::ws::WsError::Connection(format!(
                "invalid value for WebSocket header `{name}`: {error}"
            ))
        })?;
        request.headers_mut().insert(header_name, header_value);
    }

    request
        .headers()
        .iter()
        .filter(|(name, _)| *name != HOST)
        .map(|(name, value)| {
            value
                .to_str()
                .map(|value| (name.to_string(), value.to_owned()))
                .map_err(|error| {
                    cdk_http_client::ws::WsError::Connection(format!(
                        "WebSocket header `{name}` is not valid text: {error}"
                    ))
                })
        })
        .collect()
}

#[derive(Debug, Clone)]
pub(crate) struct MintTarget {
    base_url: Url,
    origin: Origin,
}

impl MintTarget {
    pub(crate) fn new(mint_url: &MintUrl) -> Result<Self> {
        let mint_url_string = mint_url.to_string();
        let base_url = Url::parse(&mint_url_string).map_err(|source| Error::InvalidMintUrl {
            url: mint_url_string,
            source,
        })?;

        if !matches!(base_url.scheme(), "http" | "https") {
            return Err(Error::UnsupportedMintScheme {
                scheme: base_url.scheme().to_owned(),
            });
        }

        if !base_url.username().is_empty() || base_url.password().is_some() {
            return Err(Error::MintUrlCredentials);
        }

        Ok(Self {
            origin: base_url.origin(),
            base_url,
        })
    }

    fn contains(&self, url: &Url) -> bool {
        url.origin() == self.origin
    }

    fn contains_websocket(&self, url: &Url) -> bool {
        let expected_scheme = match self.base_url.scheme() {
            "http" => "ws",
            "https" => "wss",
            _ => return false,
        };

        url.scheme() == expected_scheme
            && url.host() == self.base_url.host()
            && url.port_or_known_default() == self.base_url.port_or_known_default()
            && url.username().is_empty()
            && url.password().is_none()
    }

    fn request_target(&self, url: &Url) -> String {
        let mut target = url.path().to_owned();
        if target.is_empty() {
            target.push('/');
        }
        if let Some(query) = url.query() {
            target.push('?');
            target.push_str(query);
        }
        target
    }
}

/// HTTP transport that sends requests for one mint through an attested enclave.
#[derive(Clone)]
pub struct EnclaviaTransport {
    target: MintTarget,
    client: Client,
    fallback: Async,
}

impl fmt::Debug for EnclaviaTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EnclaviaTransport")
            .field("mint_url", &self.target.base_url)
            .finish_non_exhaustive()
    }
}

impl EnclaviaTransport {
    /// Construct a transport from an already-attested Enclavia client.
    pub fn new(mint_url: &MintUrl, client: Client) -> Result<Self> {
        Ok(Self::from_parts(
            MintTarget::new(mint_url)?,
            client,
            Async::default(),
        ))
    }

    /// Construct a transport with a custom fallback for non-mint requests.
    pub fn with_fallback(mint_url: &MintUrl, client: Client, fallback: Async) -> Result<Self> {
        Ok(Self::from_parts(
            MintTarget::new(mint_url)?,
            client,
            fallback,
        ))
    }

    pub(crate) fn from_parts(target: MintTarget, client: Client, fallback: Async) -> Self {
        Self {
            target,
            client,
            fallback,
        }
    }

    /// Return the underlying attested Enclavia client.
    pub fn enclavia_client(&self) -> &Client {
        &self.client
    }

    async fn enclave_request(
        &self,
        method: Method,
        url: &Url,
        auth: Option<AuthToken>,
        body: Option<(Vec<u8>, &'static str)>,
    ) -> std::result::Result<RawResponse, HttpError> {
        let target = self.target.request_target(url);
        let mut request = self.client.request(method, &target);

        if let Some(auth) = auth {
            request = request.header(auth.header_key(), auth.to_string());
        }

        if let Some((body, content_type)) = body {
            request = request.header("Content-Type", content_type).body(body);
        }

        let response = request
            .send()
            .await
            .map_err(|error| HttpError::Connection(error.to_string()))?;

        Ok(RawResponse::new(response.status(), response.into_bytes()))
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl Transport for EnclaviaTransport {
    #[cfg(not(target_arch = "wasm32"))]
    async fn ws_connect(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> std::result::Result<
        (
            cdk_http_client::ws::WsSender,
            cdk_http_client::ws::WsReceiver,
        ),
        cdk_http_client::ws::WsError,
    > {
        let parsed_url = Url::parse(url).map_err(|error| {
            cdk_http_client::ws::WsError::Connection(format!("invalid WebSocket URL: {error}"))
        })?;

        if !self.target.contains_websocket(&parsed_url) {
            return Err(cdk_http_client::ws::WsError::Connection(
                "refusing to open an Enclavia stream outside the configured mint origin".to_owned(),
            ));
        }

        let upgrade_headers = websocket_upgrade_headers(url, headers)?;
        let target = self.target.request_target(&parsed_url);
        let stream = self
            .client
            .upgrade(Method::Get, &target, &upgrade_headers)
            .await
            .map_err(|error| cdk_http_client::ws::WsError::Connection(error.to_string()))?;
        let ws_stream = WebSocketStream::from_raw_socket(stream, Role::Client, None).await;

        Ok(cdk_http_client::ws::from_websocket_stream(ws_stream))
    }

    #[cfg(target_arch = "wasm32")]
    async fn ws_connect(
        &self,
        _url: &str,
        _headers: &[(&str, &str)],
    ) -> std::result::Result<
        (
            cdk_http_client::ws::WsSender,
            cdk_http_client::ws::WsReceiver,
        ),
        cdk_http_client::ws::WsError,
    > {
        Err(cdk_http_client::ws::WsError::Connection(
            "Enclavia WebSocket subscriptions require a native target".to_owned(),
        ))
    }

    fn with_proxy(
        &mut self,
        _proxy: Url,
        _host_matcher: Option<&str>,
        _accept_invalid_certs: bool,
    ) -> std::result::Result<(), HttpError> {
        Err(HttpError::Proxy(
            "proxying an established Enclavia channel is not supported".to_owned(),
        ))
    }

    async fn resolve_dns_txt(&self, domain: &str) -> std::result::Result<Vec<String>, HttpError> {
        self.fallback.resolve_dns_txt(domain).await
    }

    async fn http_get<R>(
        &self,
        url: Url,
        auth: Option<AuthToken>,
    ) -> std::result::Result<R, HttpError>
    where
        R: DeserializeOwned,
    {
        if !self.target.contains(&url) {
            return self.fallback.http_get(url, auth).await;
        }

        self.enclave_request(Method::Get, &url, auth, None)
            .await?
            .json_or_status_error()
    }

    async fn http_get_raw(
        &self,
        url: Url,
        auth: Option<AuthToken>,
    ) -> std::result::Result<RawResponse, HttpError> {
        if !self.target.contains(&url) {
            return self.fallback.http_get_raw(url, auth).await;
        }

        self.enclave_request(Method::Get, &url, auth, None).await
    }

    async fn http_post<P, R>(
        &self,
        url: Url,
        auth: Option<AuthToken>,
        payload: &P,
    ) -> std::result::Result<R, HttpError>
    where
        P: Serialize + Send + Sync,
        R: DeserializeOwned,
    {
        if !self.target.contains(&url) {
            return self.fallback.http_post(url, auth, payload).await;
        }

        let body = serde_json::to_vec(payload)
            .map_err(|error| HttpError::Serialization(error.to_string()))?;
        self.enclave_request(Method::Post, &url, auth, Some((body, "application/json")))
            .await?
            .json_or_status_error()
    }

    async fn http_post_form_raw<P>(
        &self,
        url: Url,
        auth: Option<AuthToken>,
        payload: &P,
    ) -> std::result::Result<RawResponse, HttpError>
    where
        P: Serialize + Send + Sync,
    {
        if !self.target.contains(&url) {
            return self.fallback.http_post_form_raw(url, auth, payload).await;
        }

        let body = serde_urlencoded::to_string(payload)
            .map_err(|error| HttpError::Serialization(error.to_string()))?
            .into_bytes();
        self.enclave_request(
            Method::Post,
            &url,
            auth,
            Some((body, "application/x-www-form-urlencoded")),
        )
        .await
    }
}

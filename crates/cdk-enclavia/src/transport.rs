use core::fmt;

use async_trait::async_trait;
use cdk::mint_url::MintUrl;
use cdk::nuts::AuthToken;
use cdk_http_client::{Async, HttpError, RawResponse, Transport};
use enclavia::{Client, Method};
use serde::de::DeserializeOwned;
use serde::Serialize;
use url::{Origin, Url};

use crate::error::{Error, Result};

const WEBSOCKET_UNSUPPORTED: &str = "cdk-enclavia does not yet support NUT-17 WebSocket subscriptions; configure WalletBuilder::use_http_subscription()";

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
            WEBSOCKET_UNSUPPORTED.to_owned(),
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

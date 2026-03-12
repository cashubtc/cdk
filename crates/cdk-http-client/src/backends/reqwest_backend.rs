//! reqwest-based backend implementation

use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::HttpError;
use crate::request_builder_ext::RequestBuilderExt;
use crate::response::{RawResponse, Response};

#[derive(Debug, Clone)]
pub(crate) struct ProxyConfig {
    url: url::Url,
    matcher: Option<regex::Regex>,
}

#[derive(Clone)]
/// HTTP client wrapper backed by reqwest.
pub struct HttpClient {
    proxied: Arc<reqwest::Client>,
    direct: Arc<reqwest::Client>,
    proxy_config: Option<ProxyConfig>,
}

impl std::fmt::Debug for HttpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpClient").finish()
    }
}

impl HttpClient {
    /// Create a new HTTP client with default settings.
    pub fn new() -> Self {
        Self::builder()
            .build()
            .expect("default reqwest client should build")
    }

    /// Create a new HTTP client builder.
    pub fn builder() -> HttpClientBuilder {
        HttpClientBuilder::default()
    }

    fn from_parts(
        proxied: reqwest::Client,
        direct: reqwest::Client,
        proxy: Option<ProxyConfig>,
    ) -> Self {
        Self {
            proxied: Arc::new(proxied),
            direct: Arc::new(direct),
            proxy_config: proxy,
        }
    }

    fn use_proxy_for_url(&self, url: &str) -> bool {
        let Some(proxy) = &self.proxy_config else {
            return false;
        };

        match &proxy.matcher {
            Some(matcher) => url::Url::parse(url)
                .ok()
                .and_then(|u| u.host_str().map(|h| matcher.is_match(h)))
                .unwrap_or(false),
            None => true,
        }
    }

    fn request(&self, method: reqwest::Method, url: &str) -> reqwest::RequestBuilder {
        if self.use_proxy_for_url(url) {
            self.proxied.request(method, url)
        } else {
            self.direct.request(method, url)
        }
    }

    async fn send_json_request<R: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Response<R> {
        let response = request.send().await.map_err(map_reqwest_error)?;
        let status = response.status();
        let body = response.bytes().await.map_err(map_reqwest_error)?.to_vec();

        if !status.is_success() {
            return Err(HttpError::Status {
                status: status.as_u16(),
                message: String::from_utf8_lossy(&body).to_string(),
            });
        }

        serde_json::from_slice(&body).map_err(HttpError::from)
    }

    /// GET request, returns JSON deserialized to R.
    pub async fn fetch<R: DeserializeOwned>(&self, url: &str) -> Response<R> {
        self.send_json_request(self.request(reqwest::Method::GET, url))
            .await
    }

    /// POST with JSON body, returns JSON deserialized to R.
    pub async fn post_json<B: Serialize, R: DeserializeOwned>(
        &self,
        url: &str,
        body: &B,
    ) -> Response<R> {
        self.send_json_request(self.request(reqwest::Method::POST, url).json(body))
            .await
    }

    /// POST with form data, returns JSON deserialized to R.
    pub async fn post_form<F: Serialize, R: DeserializeOwned>(
        &self,
        url: &str,
        form: &F,
    ) -> Response<R> {
        self.send_json_request(self.request(reqwest::Method::POST, url).form(form))
            .await
    }

    /// PATCH with JSON body, returns JSON deserialized to R.
    pub async fn patch_json<B: Serialize, R: DeserializeOwned>(
        &self,
        url: &str,
        body: &B,
    ) -> Response<R> {
        self.send_json_request(self.request(reqwest::Method::PATCH, url).json(body))
            .await
    }

    /// GET request returning raw response body.
    pub async fn get_raw(&self, url: &str) -> Response<RawResponse> {
        let response = self
            .request(reqwest::Method::GET, url)
            .send()
            .await
            .map_err(map_reqwest_error)?;
        let status = response.status().as_u16();
        let body = response.bytes().await.map_err(map_reqwest_error)?.to_vec();
        Ok(RawResponse::new(status, body))
    }

    /// POST request builder for complex cases.
    pub fn post(&self, url: &str) -> ReqwestRequestBuilder {
        ReqwestRequestBuilder::new(self.request(reqwest::Method::POST, url), url)
    }

    /// GET request builder for complex cases.
    pub fn get(&self, url: &str) -> ReqwestRequestBuilder {
        ReqwestRequestBuilder::new(self.request(reqwest::Method::GET, url), url)
    }

    /// PATCH request builder for complex cases.
    pub fn patch(&self, url: &str) -> ReqwestRequestBuilder {
        ReqwestRequestBuilder::new(self.request(reqwest::Method::PATCH, url), url)
    }
}

fn map_reqwest_error(err: reqwest::Error) -> HttpError {
    if err.is_timeout() {
        HttpError::Timeout
    } else if err.is_connect() {
        HttpError::Connection(err.to_string())
    } else {
        HttpError::Other(err.to_string())
    }
}

/// reqwest-based RequestBuilder wrapper.
pub struct ReqwestRequestBuilder {
    inner: Option<reqwest::RequestBuilder>,
    error: Option<HttpError>,
    url: String,
}

impl std::fmt::Debug for ReqwestRequestBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReqwestRequestBuilder")
            .field("url", &self.url)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl ReqwestRequestBuilder {
    pub(crate) fn new(inner: reqwest::RequestBuilder, url: &str) -> Self {
        Self {
            inner: Some(inner),
            error: None,
            url: url.to_string(),
        }
    }

    fn map_inner<F>(mut self, map: F) -> Self
    where
        F: FnOnce(reqwest::RequestBuilder) -> reqwest::RequestBuilder,
    {
        if self.error.is_some() {
            return self;
        }

        if let Some(inner) = self.inner.take() {
            self.inner = Some(map(inner));
        } else {
            self.error = Some(HttpError::Other("request builder consumed".to_string()));
        }

        self
    }
}

impl RequestBuilderExt for ReqwestRequestBuilder {
    fn header(self, key: impl AsRef<str>, value: impl AsRef<str>) -> Self {
        let key = key.as_ref().to_string();
        let value = value.as_ref().to_string();
        self.map_inner(|inner| inner.header(key, value))
    }

    fn json<T: Serialize>(self, body: &T) -> Self {
        match serde_json::to_value(body) {
            Ok(value) => self.map_inner(|inner| inner.json(&value)),
            Err(e) => Self {
                inner: self.inner,
                error: Some(HttpError::Serialization(e.to_string())),
                url: self.url,
            },
        }
    }

    fn form<T: Serialize>(self, body: &T) -> Self {
        match serde_urlencoded::to_string(body) {
            Ok(form) => self.map_inner(|inner| {
                inner
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(form)
            }),
            Err(e) => Self {
                inner: self.inner,
                error: Some(HttpError::Serialization(e.to_string())),
                url: self.url,
            },
        }
    }

    async fn send(self) -> Response<RawResponse> {
        if let Some(err) = self.error {
            return Err(err);
        }

        let Some(inner) = self.inner else {
            return Err(HttpError::Other("request builder consumed".to_string()));
        };

        let response = inner.send().await.map_err(map_reqwest_error)?;
        let status = response.status().as_u16();
        let body = response.bytes().await.map_err(map_reqwest_error)?.to_vec();
        Ok(RawResponse::new(status, body))
    }

    async fn send_json<R: DeserializeOwned>(self) -> Response<R> {
        let raw = self.send().await?;
        let status = raw.status();

        if !raw.is_success() {
            let message = String::from_utf8_lossy(&raw.body).to_string();
            return Err(HttpError::Status { status, message });
        }

        serde_json::from_slice(&raw.body).map_err(HttpError::from)
    }
}

#[derive(Debug, Default)]
/// HTTP client builder for configuring proxy and TLS settings.
pub struct HttpClientBuilder {
    proxy: Option<ProxyConfig>,
    accept_invalid_certs: bool,
}

impl HttpClientBuilder {
    /// Accept invalid TLS certificates.
    pub fn danger_accept_invalid_certs(mut self, accept: bool) -> Self {
        self.accept_invalid_certs = accept;
        self
    }

    /// Set a proxy URL.
    pub fn proxy(mut self, url: url::Url) -> Self {
        self.proxy = Some(ProxyConfig { url, matcher: None });
        self
    }

    /// Set a proxy URL with a host pattern matcher.
    pub fn proxy_with_matcher(mut self, url: url::Url, pattern: &str) -> Response<Self> {
        let matcher = regex::Regex::new(pattern)
            .map_err(|e| HttpError::Proxy(format!("Invalid proxy pattern: {}", e)))?;
        self.proxy = Some(ProxyConfig {
            url,
            matcher: Some(matcher),
        });
        Ok(self)
    }

    /// Build the HTTP client.
    pub fn build(self) -> Response<HttpClient> {
        let direct = reqwest::Client::builder()
            .danger_accept_invalid_certs(self.accept_invalid_certs)
            .build()
            .map_err(|e| HttpError::Build(e.to_string()))?;

        let proxied = if let Some(proxy) = &self.proxy {
            reqwest::Client::builder()
                .danger_accept_invalid_certs(self.accept_invalid_certs)
                .proxy(
                    reqwest::Proxy::all(proxy.url.as_str())
                        .map_err(|e| HttpError::Proxy(e.to_string()))?,
                )
                .build()
                .map_err(|e| HttpError::Build(e.to_string()))?
        } else {
            direct.clone()
        };

        Ok(HttpClient::from_parts(proxied, direct, self.proxy))
    }
}

//! bitreq-based backend implementation

use std::sync::Arc;

use bitreq::RequestExt;
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

pub(crate) fn apply_proxy_if_needed(
    request: bitreq::Request,
    url: &str,
    proxy_config: &Option<ProxyConfig>,
) -> Response<bitreq::Request> {
    if let Some(ref config) = proxy_config {
        if let Some(ref matcher) = config.matcher {
            if let Ok(parsed_url) = url::Url::parse(url) {
                if let Some(host) = parsed_url.host_str() {
                    if matcher.is_match(host) {
                        let proxy = bitreq::Proxy::new_http(&config.url)
                            .map_err(|e| HttpError::Proxy(e.to_string()))?;
                        return Ok(request.with_proxy(proxy));
                    }
                }
            }
        } else {
            let proxy = bitreq::Proxy::new_http(&config.url)
                .map_err(|e| HttpError::Proxy(e.to_string()))?;
            return Ok(request.with_proxy(proxy));
        }
    }
    Ok(request)
}

/// HTTP client wrapper
#[derive(Clone)]
pub struct HttpClient {
    inner: Arc<bitreq::Client>,
    proxy_config: Option<ProxyConfig>,
}

impl std::fmt::Debug for HttpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpClient").finish()
    }
}

impl HttpClient {
    /// Create a new HTTP client with default settings
    pub fn new() -> Self {
        Self {
            inner: Arc::new(bitreq::Client::new(10)),
            proxy_config: None,
        }
    }

    /// Create an HTTP client from pre-built parts
    pub(crate) fn from_parts(
        client: Arc<bitreq::Client>,
        proxy_config: Option<ProxyConfig>,
    ) -> Self {
        Self {
            inner: client,
            proxy_config,
        }
    }

    /// Create a new HTTP client builder
    pub fn builder() -> HttpClientBuilder {
        HttpClientBuilder::default()
    }

    /// Helper method to apply proxy if URL matches the configured proxy rules
    fn apply_proxy_if_needed(
        &self,
        request: bitreq::Request,
        url: &str,
    ) -> Response<bitreq::Request> {
        apply_proxy_if_needed(request, url, &self.proxy_config)
    }

    /// GET request, returns JSON deserialized to R
    pub async fn fetch<R: DeserializeOwned>(&self, url: &str) -> Response<R> {
        let request = bitreq::get(url);
        let request = self.apply_proxy_if_needed(request, url)?;
        let response = request
            .send_async_with_client(&self.inner)
            .await
            .map_err(HttpError::from)?;
        let status = response.status_code;

        if !(200..300).contains(&status) {
            let message = response.as_str().unwrap_or("").to_string();
            return Err(HttpError::Status {
                status: status as u16,
                message,
            });
        }

        response.json().map_err(HttpError::from)
    }

    /// POST with JSON body, returns JSON deserialized to R
    pub async fn post_json<B: Serialize, R: DeserializeOwned>(
        &self,
        url: &str,
        body: &B,
    ) -> Response<R> {
        let request = bitreq::post(url).with_json(body).map_err(HttpError::from)?;
        let request = self.apply_proxy_if_needed(request, url)?;
        let response: bitreq::Response = request
            .send_async_with_client(&self.inner)
            .await
            .map_err(HttpError::from)?;
        let status = response.status_code;

        if !(200..300).contains(&status) {
            let message = response.as_str().unwrap_or("").to_string();
            return Err(HttpError::Status {
                status: status as u16,
                message,
            });
        }

        response.json().map_err(HttpError::from)
    }

    /// POST with form data, returns JSON deserialized to R
    pub async fn post_form<F: Serialize, R: DeserializeOwned>(
        &self,
        url: &str,
        form: &F,
    ) -> Response<R> {
        let form_str = serde_urlencoded::to_string(form)
            .map_err(|e| HttpError::Serialization(e.to_string()))?;
        let request = bitreq::post(url)
            .with_body(form_str.into_bytes())
            .with_header("Content-Type", "application/x-www-form-urlencoded");
        let request = self.apply_proxy_if_needed(request, url)?;
        let response: bitreq::Response = request
            .send_async_with_client(&self.inner)
            .await
            .map_err(HttpError::from)?;
        let status = response.status_code;

        if !(200..300).contains(&status) {
            let message = response.as_str().unwrap_or("").to_string();
            return Err(HttpError::Status {
                status: status as u16,
                message,
            });
        }

        response.json().map_err(HttpError::from)
    }

    /// PATCH with JSON body, returns JSON deserialized to R
    pub async fn patch_json<B: Serialize, R: DeserializeOwned>(
        &self,
        url: &str,
        body: &B,
    ) -> Response<R> {
        let request = bitreq::patch(url)
            .with_json(body)
            .map_err(HttpError::from)?;
        let request = self.apply_proxy_if_needed(request, url)?;
        let response: bitreq::Response = request
            .send_async_with_client(&self.inner)
            .await
            .map_err(HttpError::from)?;
        let status = response.status_code;

        if !(200..300).contains(&status) {
            let message = response.as_str().unwrap_or("").to_string();
            return Err(HttpError::Status {
                status: status as u16,
                message,
            });
        }

        response.json().map_err(HttpError::from)
    }

    /// GET request returning raw response body
    pub async fn get_raw(&self, url: &str) -> Response<RawResponse> {
        let request = bitreq::get(url);
        let request = self.apply_proxy_if_needed(request, url)?;
        let response = request
            .send_async_with_client(&self.inner)
            .await
            .map_err(HttpError::from)?;
        Ok(RawResponse::new(response))
    }

    /// POST request builder for complex cases
    pub fn post(&self, url: &str) -> BitreqRequestBuilder {
        BitreqRequestBuilder::new(
            bitreq::post(url),
            url,
            self.inner.clone(),
            self.proxy_config.clone(),
        )
    }

    /// GET request builder for complex cases
    pub fn get(&self, url: &str) -> BitreqRequestBuilder {
        BitreqRequestBuilder::new(
            bitreq::get(url),
            url,
            self.inner.clone(),
            self.proxy_config.clone(),
        )
    }

    /// PATCH request builder for complex cases
    pub fn patch(&self, url: &str) -> BitreqRequestBuilder {
        BitreqRequestBuilder::new(
            bitreq::patch(url),
            url,
            self.inner.clone(),
            self.proxy_config.clone(),
        )
    }
}

/// bitreq-based RequestBuilder wrapper
pub struct BitreqRequestBuilder {
    inner: bitreq::Request,
    error: Option<HttpError>,
    url: String,
    client: Arc<bitreq::Client>,
    proxy_config: Option<ProxyConfig>,
}

impl std::fmt::Debug for BitreqRequestBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BitreqRequestBuilder")
            .field("url", &self.url)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl BitreqRequestBuilder {
    /// Create a new BitreqRequestBuilder from a bitreq::Request
    pub(crate) fn new(
        inner: bitreq::Request,
        url: &str,
        client: Arc<bitreq::Client>,
        proxy_config: Option<ProxyConfig>,
    ) -> Self {
        Self {
            inner,
            error: None,
            url: url.to_string(),
            client,
            proxy_config,
        }
    }
}

impl RequestBuilderExt for BitreqRequestBuilder {
    fn header(self, key: impl AsRef<str>, value: impl AsRef<str>) -> Self {
        Self {
            inner: self.inner.with_header(key.as_ref(), value.as_ref()),
            error: self.error,
            url: self.url,
            client: self.client,
            proxy_config: self.proxy_config,
        }
    }

    fn json<T: Serialize>(mut self, body: &T) -> Self {
        match self.inner.clone().with_json(body) {
            Ok(req) => {
                self.inner = req;
                self.error = None;
            }
            Err(e) => self.error = Some(HttpError::from(e)),
        }
        self
    }

    fn form<T: Serialize>(mut self, body: &T) -> Self {
        match serde_urlencoded::to_string(body) {
            Ok(form_str) => {
                self.inner = self
                    .inner
                    .with_body(form_str.into_bytes())
                    .with_header("Content-Type", "application/x-www-form-urlencoded");
            }
            Err(e) => self.error = Some(HttpError::Serialization(e.to_string())),
        }
        self
    }

    async fn send(self) -> Response<RawResponse> {
        if let Some(err) = self.error {
            return Err(err);
        }
        let request = apply_proxy_if_needed(self.inner, &self.url, &self.proxy_config)?;
        let response = request
            .send_async_with_client(&self.client)
            .await
            .map_err(HttpError::from)?;
        Ok(RawResponse::new(response))
    }

    async fn send_json<R: DeserializeOwned>(self) -> Response<R> {
        if let Some(err) = self.error {
            return Err(err);
        }
        let request = apply_proxy_if_needed(self.inner, &self.url, &self.proxy_config)?;
        let response = request
            .send_async_with_client(&self.client)
            .await
            .map_err(HttpError::from)?;
        let status = response.status_code;

        if !(200..300).contains(&status) {
            let message = response.as_str().unwrap_or("").to_string();
            return Err(HttpError::Status {
                status: status as u16,
                message,
            });
        }

        response.json().map_err(HttpError::from)
    }
}

/// HTTP client builder for configuring proxy and TLS settings
#[derive(Debug, Default)]
pub struct HttpClientBuilder {
    proxy: Option<ProxyConfig>,
    accept_invalid_certs: bool,
}

impl HttpClientBuilder {
    /// Accept invalid TLS certificates
    pub fn danger_accept_invalid_certs(mut self, accept: bool) -> Self {
        self.accept_invalid_certs = accept;
        self
    }

    /// Set a proxy URL
    pub fn proxy(mut self, url: url::Url) -> Self {
        self.proxy = Some(ProxyConfig { url, matcher: None });
        self
    }

    /// Set a proxy URL with a host pattern matcher
    pub fn proxy_with_matcher(mut self, url: url::Url, pattern: &str) -> Response<Self> {
        let matcher = regex::Regex::new(pattern)
            .map_err(|e| HttpError::Proxy(format!("Invalid proxy pattern: {}", e)))?;
        self.proxy = Some(ProxyConfig {
            url,
            matcher: Some(matcher),
        });
        Ok(self)
    }

    /// Build the HTTP client
    pub fn build(self) -> Response<HttpClient> {
        if self.accept_invalid_certs {
            return Err(HttpError::Build(
                "danger_accept_invalid_certs is not supported".to_string(),
            ));
        }

        Ok(HttpClient::from_parts(
            Arc::new(bitreq::Client::new(10)),
            self.proxy,
        ))
    }
}

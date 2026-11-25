//! bitreq-based backend implementation

use serde::de::DeserializeOwned;
use serde::Serialize;

use super::url_for_debug;
use crate::error::HttpError;
use crate::response::{RawResponse, Response};

/// Default per-request timeout for the bitreq backend, in seconds.
const DEFAULT_BITREQ_REQUEST_TIMEOUT_SECS: u64 = 300;

#[derive(Clone)]
pub(crate) struct ProxyConfig {
    url: url::Url,
    matcher: Option<regex::Regex>,
}

impl std::fmt::Debug for ProxyConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyConfig")
            .field("url", &url_for_debug(self.url.as_str()))
            .field("matcher", &self.matcher)
            .finish()
    }
}

fn validate_proxy_url(url: &url::Url) -> Response<()> {
    match url.scheme() {
        "http" => Ok(()),
        scheme => Err(HttpError::Proxy(format!(
            "Unsupported proxy URL scheme for bitreq backend: {scheme}"
        ))),
    }
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
    proxy_config: Option<ProxyConfig>,
    no_redirects: bool,
    request_timeout_secs: Option<u64>,
}

impl std::fmt::Debug for HttpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpClient").finish()
    }
}

impl HttpClient {
    /// Create a new HTTP client with default settings
    pub fn new() -> Self {
        super::install_rustls_crypto_provider();
        Self {
            proxy_config: None,
            no_redirects: false,
            request_timeout_secs: Some(DEFAULT_BITREQ_REQUEST_TIMEOUT_SECS),
        }
    }

    /// Create an HTTP client from pre-built parts
    pub(crate) fn from_parts(
        proxy_config: Option<ProxyConfig>,
        no_redirects: bool,
        request_timeout_secs: Option<u64>,
    ) -> Self {
        super::install_rustls_crypto_provider();
        Self {
            proxy_config,
            no_redirects,
            request_timeout_secs,
        }
    }

    /// Create a new HTTP client builder
    pub fn builder() -> HttpClientBuilder {
        HttpClientBuilder::default()
    }

    /// GET request, returning deserialized JSON.
    pub async fn fetch<R>(&self, url: &str) -> Response<R>
    where
        R: DeserializeOwned,
    {
        self.get(url).send_json().await
    }

    /// POST with JSON body, returning deserialized JSON.
    pub async fn post_json<B, R>(&self, url: &str, body: &B) -> Response<R>
    where
        B: Serialize,
        R: DeserializeOwned,
    {
        self.post(url).json(body).send_json().await
    }

    /// POST with form data, returning deserialized JSON.
    pub async fn post_form<F, R>(&self, url: &str, form: &F) -> Response<R>
    where
        F: Serialize,
        R: DeserializeOwned,
    {
        self.post(url).form(form).send_json().await
    }

    /// PATCH with JSON body, returning deserialized JSON.
    pub async fn patch_json<B, R>(&self, url: &str, body: &B) -> Response<R>
    where
        B: Serialize,
        R: DeserializeOwned,
    {
        self.patch(url).json(body).send_json().await
    }

    /// GET request returning the raw response body.
    pub async fn get_raw(&self, url: &str) -> Response<RawResponse> {
        self.get(url).send().await
    }

    /// POST request builder for complex cases
    pub fn post(&self, url: &str) -> BitreqRequestBuilder {
        BitreqRequestBuilder::new(
            bitreq::post(url),
            url,
            self.proxy_config.clone(),
            self.no_redirects,
            self.request_timeout_secs,
        )
    }

    /// GET request builder for complex cases
    pub fn get(&self, url: &str) -> BitreqRequestBuilder {
        BitreqRequestBuilder::new(
            bitreq::get(url),
            url,
            self.proxy_config.clone(),
            self.no_redirects,
            self.request_timeout_secs,
        )
    }

    /// PATCH request builder for complex cases
    pub fn patch(&self, url: &str) -> BitreqRequestBuilder {
        BitreqRequestBuilder::new(
            bitreq::patch(url),
            url,
            self.proxy_config.clone(),
            self.no_redirects,
            self.request_timeout_secs,
        )
    }

    /// PUT request builder for complex cases
    pub fn put(&self, url: &str) -> BitreqRequestBuilder {
        BitreqRequestBuilder::new(
            bitreq::put(url),
            url,
            self.proxy_config.clone(),
            self.no_redirects,
            self.request_timeout_secs,
        )
    }
}

/// bitreq-based RequestBuilder wrapper
pub struct BitreqRequestBuilder {
    inner: bitreq::Request,
    error: Option<HttpError>,
    url: String,
    proxy_config: Option<ProxyConfig>,
    no_redirects: bool,
    request_timeout_secs: Option<u64>,
}

impl std::fmt::Debug for BitreqRequestBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BitreqRequestBuilder")
            .field("url", &url_for_debug(&self.url))
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl BitreqRequestBuilder {
    /// Create a new BitreqRequestBuilder from a bitreq::Request
    pub(crate) fn new(
        inner: bitreq::Request,
        url: &str,
        proxy_config: Option<ProxyConfig>,
        no_redirects: bool,
        request_timeout_secs: Option<u64>,
    ) -> Self {
        Self {
            inner,
            error: None,
            url: url.to_string(),
            proxy_config,
            no_redirects,
            request_timeout_secs,
        }
    }
    /// Add a header to the request.
    pub fn header(self, key: impl AsRef<str>, value: impl AsRef<str>) -> Self {
        Self {
            inner: self.inner.with_header(key.as_ref(), value.as_ref()),
            error: self.error,
            url: self.url,
            proxy_config: self.proxy_config,
            no_redirects: self.no_redirects,
            request_timeout_secs: self.request_timeout_secs,
        }
    }

    /// Set the request body as JSON.
    pub fn json<T>(mut self, body: &T) -> Self
    where
        T: Serialize,
    {
        // Preserve any error already set by an earlier builder step rather than
        // clearing it on a successful serialization.
        if self.error.is_some() {
            return self;
        }
        match self.inner.clone().with_json(body) {
            Ok(req) => self.inner = req,
            Err(e) => self.error = Some(HttpError::from(e)),
        }
        self
    }

    /// Set the request body as form data.
    pub fn form<T>(mut self, body: &T) -> Self
    where
        T: Serialize + ?Sized,
    {
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

    /// Send the request and return a raw response.
    pub async fn send(self) -> Response<RawResponse> {
        if let Some(err) = self.error {
            return Err(err);
        }
        let request = apply_proxy_if_needed(self.inner, &self.url, &self.proxy_config)?;
        let request = if self.no_redirects {
            request.with_max_redirects(0)
        } else {
            request
        };
        let request = match self.request_timeout_secs {
            Some(timeout_secs) => request.with_timeout(timeout_secs),
            None => request,
        }
        .with_max_body_size(crate::MAX_RESPONSE_BYTES);
        // bitreq's blocking client opens a fresh connection for every request
        // and enforces the timeout, so a stale pooled connection cannot stall
        // the caller. Run it off the async runtime.
        let response = tokio::task::spawn_blocking(move || request.send())
            .await
            .map_err(|err| HttpError::Other(format!("HTTP request task failed: {err}")))?
            .map_err(HttpError::from)?;
        Ok(RawResponse::new(
            response.status_code as u16,
            response.into_bytes(),
        ))
    }

    /// Send the request and deserialize the response as JSON.
    pub async fn send_json<R>(self) -> Response<R>
    where
        R: DeserializeOwned,
    {
        self.send().await?.json_or_status_error()
    }
}

/// HTTP client builder for configuring proxy and TLS settings
#[derive(Debug)]
pub struct HttpClientBuilder {
    proxy: Option<ProxyConfig>,
    accept_invalid_certs: bool,
    no_redirects: bool,
    request_timeout_secs: Option<u64>,
}

impl Default for HttpClientBuilder {
    fn default() -> Self {
        Self {
            proxy: None,
            accept_invalid_certs: false,
            no_redirects: false,
            request_timeout_secs: Some(DEFAULT_BITREQ_REQUEST_TIMEOUT_SECS),
        }
    }
}

impl HttpClientBuilder {
    /// Accept invalid TLS certificates
    pub fn danger_accept_invalid_certs(mut self, accept: bool) -> Self {
        self.accept_invalid_certs = accept;
        self
    }

    /// Disable automatic HTTP redirect following
    pub fn no_redirects(mut self) -> Self {
        self.no_redirects = true;
        self
    }

    /// Set the per-request timeout in seconds, or `None` for no timeout.
    pub fn request_timeout_secs(mut self, timeout_secs: Option<u64>) -> Self {
        self.request_timeout_secs = timeout_secs;
        self
    }

    /// Set an HTTP proxy URL.
    ///
    /// The `bitreq` backend supports HTTP proxy URLs only. SOCKS proxy schemes
    /// such as `socks5h` require building this crate with the `reqwest` feature.
    pub fn proxy(mut self, url: url::Url) -> Self {
        self.proxy = Some(ProxyConfig { url, matcher: None });
        self
    }

    /// Set an HTTP proxy URL with a host pattern matcher.
    ///
    /// The `bitreq` backend supports HTTP proxy URLs only. SOCKS proxy schemes
    /// such as `socks5h` require building this crate with the `reqwest` feature.
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

        if let Some(proxy) = &self.proxy {
            validate_proxy_url(&proxy.url)?;
        }

        Ok(HttpClient::from_parts(
            self.proxy,
            self.no_redirects,
            self.request_timeout_secs,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::HttpClient;

    #[test]
    fn request_builder_debug_redacts_url_credentials() {
        let secret = "bitreq-url-token-secret";
        let url = format!("https://user:{secret}@mint.example.com/api?token={secret}");
        let request = HttpClient::new().post(&url);

        let debug = format!("{request:?}");

        assert!(debug.contains("https://mint.example.com/api"));
        assert!(!debug.contains(secret));
    }

    #[test]
    fn client_builder_debug_redacts_proxy_credentials() {
        let secret = "bitreq-proxy-password-secret";
        let proxy = url::Url::parse(&format!("http://user:{secret}@proxy.example.com:8080"))
            .expect("valid proxy URL");
        let builder = HttpClient::builder().proxy(proxy);

        let debug = format!("{builder:?}");

        assert!(debug.contains("http://proxy.example.com:8080"));
        assert!(!debug.contains(secret));
    }
}

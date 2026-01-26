//! HTTP client wrapper

use serde::de::DeserializeOwned;
use serde::Serialize;

use super::error::HttpError;
use super::request::RequestBuilder;
use super::response::{RawResponse, Response};

/// HTTP client wrapper
#[derive(Debug, Clone)]
pub struct HttpClient {
    inner: reqwest::Client,
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpClient {
    /// Create a new HTTP client with default settings
    pub fn new() -> Self {
        Self {
            inner: reqwest::Client::new(),
        }
    }

    /// Create a new HTTP client builder
    pub fn builder() -> HttpClientBuilder {
        HttpClientBuilder::default()
    }

    /// Create an HttpClient from a reqwest::Client
    pub fn from_reqwest(client: reqwest::Client) -> Self {
        Self { inner: client }
    }

    // === Simple convenience methods ===

    /// GET request, returns JSON deserialized to R
    pub async fn fetch<R>(&self, url: &str) -> Response<R>
    where
        R: DeserializeOwned,
    {
        let response = self.inner.get(url).send().await?;
        let status = response.status();

        if !status.is_success() {
            let message = response.text().await.unwrap_or_default();
            return Err(HttpError::Status {
                status: status.as_u16(),
                message,
            });
        }

        response.json().await.map_err(HttpError::from)
    }

    /// POST with JSON body, returns JSON deserialized to R
    pub async fn post_json<B, R>(&self, url: &str, body: &B) -> Response<R>
    where
        B: Serialize + ?Sized,
        R: DeserializeOwned,
    {
        let response = self.inner.post(url).json(body).send().await?;
        let status = response.status();

        if !status.is_success() {
            let message = response.text().await.unwrap_or_default();
            return Err(HttpError::Status {
                status: status.as_u16(),
                message,
            });
        }

        response.json().await.map_err(HttpError::from)
    }

    /// POST with form data, returns JSON deserialized to R
    pub async fn post_form<F, R>(&self, url: &str, form: &F) -> Response<R>
    where
        F: Serialize + ?Sized,
        R: DeserializeOwned,
    {
        let response = self.inner.post(url).form(form).send().await?;
        let status = response.status();

        if !status.is_success() {
            let message = response.text().await.unwrap_or_default();
            return Err(HttpError::Status {
                status: status.as_u16(),
                message,
            });
        }

        response.json().await.map_err(HttpError::from)
    }

    /// PATCH with JSON body, returns JSON deserialized to R
    pub async fn patch_json<B, R>(&self, url: &str, body: &B) -> Response<R>
    where
        B: Serialize + ?Sized,
        R: DeserializeOwned,
    {
        let response = self.inner.patch(url).json(body).send().await?;
        let status = response.status();

        if !status.is_success() {
            let message = response.text().await.unwrap_or_default();
            return Err(HttpError::Status {
                status: status.as_u16(),
                message,
            });
        }

        response.json().await.map_err(HttpError::from)
    }

    // === Raw request methods ===

    /// GET request returning raw response body
    pub async fn get_raw(&self, url: &str) -> Response<RawResponse> {
        let response = self.inner.get(url).send().await?;
        Ok(RawResponse::new(response))
    }

    // === Request builder methods ===

    /// POST request builder for complex cases (custom headers, form data, etc.)
    pub fn post(&self, url: &str) -> RequestBuilder {
        RequestBuilder::new(self.inner.post(url))
    }

    /// GET request builder for complex cases (custom headers, etc.)
    pub fn get(&self, url: &str) -> RequestBuilder {
        RequestBuilder::new(self.inner.get(url))
    }

    /// PATCH request builder for complex cases (custom headers, JSON body, etc.)
    pub fn patch(&self, url: &str) -> RequestBuilder {
        RequestBuilder::new(self.inner.patch(url))
    }
}

/// HTTP client builder for configuring proxy and TLS settings
#[derive(Debug, Default)]
pub struct HttpClientBuilder {
    #[cfg(not(target_arch = "wasm32"))]
    accept_invalid_certs: bool,
    #[cfg(not(target_arch = "wasm32"))]
    proxy: Option<ProxyConfig>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
struct ProxyConfig {
    url: url::Url,
    matcher: Option<regex::Regex>,
}

impl HttpClientBuilder {
    /// Accept invalid TLS certificates (non-WASM only)
    #[cfg(not(target_arch = "wasm32"))]
    pub fn danger_accept_invalid_certs(mut self, accept: bool) -> Self {
        self.accept_invalid_certs = accept;
        self
    }

    /// Set a proxy URL (non-WASM only)
    #[cfg(not(target_arch = "wasm32"))]
    pub fn proxy(mut self, url: url::Url) -> Self {
        self.proxy = Some(ProxyConfig { url, matcher: None });
        self
    }

    /// Set a proxy URL with a host pattern matcher (non-WASM only)
    #[cfg(not(target_arch = "wasm32"))]
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
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut builder =
                reqwest::Client::builder().danger_accept_invalid_certs(self.accept_invalid_certs);

            if let Some(proxy_config) = self.proxy {
                let proxy_url = proxy_config.url.to_string();
                let proxy = if let Some(matcher) = proxy_config.matcher {
                    reqwest::Proxy::custom(move |url| {
                        if matcher.is_match(url.host_str().unwrap_or("")) {
                            Some(proxy_url.clone())
                        } else {
                            None
                        }
                    })
                } else {
                    reqwest::Proxy::all(&proxy_url).map_err(|e| HttpError::Proxy(e.to_string()))?
                };
                builder = builder.proxy(proxy);
            }

            let client = builder.build().map_err(HttpError::from)?;
            Ok(HttpClient { inner: client })
        }

        #[cfg(target_arch = "wasm32")]
        {
            Ok(HttpClient::new())
        }
    }
}

/// Convenience function for simple GET requests (replaces reqwest::get)
pub async fn fetch<R: DeserializeOwned>(url: &str) -> Response<R> {
    HttpClient::new().fetch(url).await
}

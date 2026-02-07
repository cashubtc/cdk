//! HTTP client wrapper

use serde::de::DeserializeOwned;
use serde::Serialize;

#[cfg(not(target_arch = "wasm32"))]
use bitreq::RequestExt;

use crate::error::HttpError;
use crate::request::RequestBuilder;
use crate::response::{RawResponse, Response};

/// HTTP client wrapper
#[derive(Clone)]
pub struct HttpClient {
    #[cfg(target_arch = "wasm32")]
    inner: reqwest::Client,
    #[cfg(not(target_arch = "wasm32"))]
    inner: bitreq::Client,
    #[cfg(not(target_arch = "wasm32"))]
    proxy_config: Option<ProxyConfig>,
}

impl std::fmt::Debug for HttpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpClient").finish()
    }
}

// #[cfg(not(target_arch = "wasm32"))]
// impl std::fmt::Debug for HttpClient {
//     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//         f.debug_struct("HttpClient").finish()
//     }
// }

#[cfg(target_arch = "wasm32")]
impl HttpClient {
    /// Create a new HTTP client with default settings
    pub fn new() -> Self {
        Self {
            inner: reqwest::Client::new(),
        }
    }

    /// Create an HttpClient from a reqwest::Client
    pub fn from_reqwest(client: reqwest::Client) -> Self {
        Self { inner: client }
    }

    /// Create a new HTTP client builder
    pub fn builder() -> HttpClientBuilder {
        HttpClientBuilder::default()
    }

    /// GET request, returns JSON deserialized to R
    pub async fn fetch<R: DeserializeOwned>(&self, url: &str) -> Response<R> {
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
    pub async fn post_json<B: Serialize + ?Sized, R: DeserializeOwned>(
        &self,
        url: &str,
        body: &B,
    ) -> Response<R> {
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
    pub async fn post_form<F: Serialize + ?Sized, R: DeserializeOwned>(
        &self,
        url: &str,
        form: &F,
    ) -> Response<R> {
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
    pub async fn patch_json<B: Serialize + ?Sized, R: DeserializeOwned>(
        &self,
        url: &str,
        body: &B,
    ) -> Response<R> {
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

    /// GET request returning raw response body
    pub async fn get_raw(&self, url: &str) -> Response<RawResponse> {
        let response = self.inner.get(url).send().await?;
        Ok(RawResponse::new(response))
    }

    /// POST request builder for complex cases
    pub fn post(&self, url: &str) -> RequestBuilder {
        RequestBuilder::new(self.inner.post(url))
    }

    /// GET request builder for complex cases
    pub fn get(&self, url: &str) -> RequestBuilder {
        RequestBuilder::new(self.inner.get(url))
    }

    /// PATCH request builder for complex cases
    pub fn patch(&self, url: &str) -> RequestBuilder {
        RequestBuilder::new(self.inner.patch(url))
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl HttpClient {
    /// Create a new HTTP client with default settings
    pub fn new() -> Self {
        Self {
            inner: bitreq::Client::new(10), // Default capacity of 10
            proxy_config: None,
        }
    }

    /// Create a new HTTP client builder
    pub fn builder() -> HttpClientBuilder {
        HttpClientBuilder::default()
    }

    /// Helper method to apply proxy if URL matches the configured proxy rules
    fn apply_proxy_if_needed(&self, request: bitreq::Request, url: &str) -> Response<bitreq::Request> {
        if let Some(ref config) = self.proxy_config {
            if let Some(ref matcher) = config.matcher {
                // Check if URL host matches the regex pattern
                if let Ok(parsed_url) = url::Url::parse(url) {
                    if let Some(host) = parsed_url.host_str() {
                        if matcher.is_match(host) {
                            let proxy = bitreq::Proxy::new_http(&config.url.to_string())
                                .map_err(|e| HttpError::Proxy(e.to_string()))?;
                            return Ok(request.with_proxy(proxy));
                        }
                    }
                }
            } else {
                // No matcher, apply proxy to all requests
                let proxy = bitreq::Proxy::new_http(&config.url.to_string())
                    .map_err(|e| HttpError::Proxy(e.to_string()))?;
                return Ok(request.with_proxy(proxy));
            }
        }
        Ok(request)
    }

    /// GET request, returns JSON deserialized to R
    pub async fn fetch<R: DeserializeOwned>(&self, url: &str) -> Response<R> {
        let request = bitreq::get(url);
        let request = self.apply_proxy_if_needed(request, url)?;
        let response = request.send_async_with_client(&self.inner).await.map_err(HttpError::from)?;
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
        let request = bitreq::post(url)
            .with_json(body)
            .map_err(HttpError::from)?;
        let request = self.apply_proxy_if_needed(request, url)?;
        let response: bitreq::Response = request.send_async_with_client(&self.inner).await.map_err(HttpError::from)?;
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
        let request = bitreq::post(url).with_body(form_str.into_bytes());
        let request = self.apply_proxy_if_needed(request, url)?;
        let response: bitreq::Response = request.send_async_with_client(&self.inner).await.map_err(HttpError::from)?;
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
        let response: bitreq::Response = request.send_async_with_client(&self.inner).await.map_err(HttpError::from)?;
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
        let response = request.send_async_with_client(&self.inner).await.map_err(HttpError::from)?;
        Ok(RawResponse::new(response))
    }

    /// POST request builder for complex cases
    pub fn post(&self, url: &str) -> RequestBuilder {
        // Note: Proxy will be applied when the request is sent
        RequestBuilder::new(bitreq::post(url))
    }

    /// GET request builder for complex cases
    pub fn get(&self, url: &str) -> RequestBuilder {
        RequestBuilder::new(bitreq::get(url))
    }

    /// PATCH request builder for complex cases
    pub fn patch(&self, url: &str) -> RequestBuilder {
        RequestBuilder::new(bitreq::patch(url))
    }
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new()
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
#[derive(Debug, Clone)]
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
        #[cfg(target_arch = "wasm32")]
        {
            Ok(HttpClient {
                inner: reqwest::Client::new(),
            })
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            // Return error if danger_accept_invalid_certs was set on non-wasm32
            if self.accept_invalid_certs {
                return Err(HttpError::Build(
                    "danger_accept_invalid_certs is not supported on non-WASM targets".to_string(),
                ));
            }

            Ok(HttpClient {
                inner: bitreq::Client::new(10), // Default capacity of 10
                proxy_config: self.proxy,
            })
        }
    }
}

/// Convenience function for simple GET requests
pub async fn fetch<R: DeserializeOwned>(url: &str) -> Response<R> {
    HttpClient::new().fetch(url).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_new() {
        let client = HttpClient::new();
        // Client should be constructable without panicking
        let _ = format!("{:?}", client);
    }

    #[test]
    fn test_client_default() {
        let client = HttpClient::default();
        // Default should produce a valid client
        let _ = format!("{:?}", client);
    }

    #[test]
    fn test_builder_returns_builder() {
        let builder = HttpClient::builder();
        let _ = format!("{:?}", builder);
    }

    #[test]
    fn test_builder_build() {
        let result = HttpClientBuilder::default().build();
        assert!(result.is_ok());
    }

    #[cfg(target_arch = "wasm32")]
    #[test]
    fn test_from_reqwest() {
        let reqwest_client = reqwest::Client::new();
        let client = HttpClient::from_reqwest(reqwest_client);
        let _ = format!("{:?}", client);
    }

    #[cfg(not(target_arch = "wasm32"))]
    mod non_wasm {
        use super::*;

        #[test]
        fn test_builder_accept_invalid_certs_returns_error() {
            let result = HttpClientBuilder::default()
                .danger_accept_invalid_certs(true)
                .build();
            assert!(result.is_err());
            if let Err(HttpError::Build(msg)) = result {
                assert!(msg.contains("danger_accept_invalid_certs"));
            } else {
                panic!("Expected HttpError::Build");
            }
        }

        #[test]
        fn test_builder_accept_invalid_certs_false_ok() {
            let result = HttpClientBuilder::default()
                .danger_accept_invalid_certs(false)
                .build();
            assert!(result.is_ok());
        }

        #[test]
        fn test_builder_proxy() {
            let proxy_url = url::Url::parse("http://localhost:8080").expect("Valid proxy URL");
            let result = HttpClientBuilder::default().proxy(proxy_url).build();
            assert!(result.is_ok());
        }

        #[test]
        fn test_builder_proxy_with_valid_matcher() {
            let proxy_url = url::Url::parse("http://localhost:8080").expect("Valid proxy URL");
            let result =
                HttpClientBuilder::default().proxy_with_matcher(proxy_url, r".*\.example\.com$");
            assert!(result.is_ok());

            let builder = result.expect("Valid matcher should succeed");
            let client_result = builder.build();
            assert!(client_result.is_ok());
        }

        #[test]
        fn test_builder_proxy_with_invalid_matcher() {
            let proxy_url = url::Url::parse("http://localhost:8080").expect("Valid proxy URL");
            // Invalid regex pattern (unclosed bracket)
            let result = HttpClientBuilder::default().proxy_with_matcher(proxy_url, r"[invalid");
            assert!(result.is_err());

            if let Err(HttpError::Proxy(msg)) = result {
                assert!(msg.contains("Invalid proxy pattern"));
            } else {
                panic!("Expected HttpError::Proxy");
            }
        }
    }
}

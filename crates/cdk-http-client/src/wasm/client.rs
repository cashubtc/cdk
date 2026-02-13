//! WASM HTTP client using the browser's native `fetch()` API

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::wasm::request::RequestBuilder;
use crate::wasm::response::RawResponse;
use crate::Response;

/// HTTP client wrapper
#[derive(Debug, Clone)]
pub struct HttpClient;

impl Default for HttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpClient {
    /// Create a new HTTP client with default settings
    pub fn new() -> Self {
        Self
    }

    /// Create a new HTTP client builder
    pub fn builder() -> HttpClientBuilder {
        HttpClientBuilder::default()
    }

    // === Simple convenience methods ===

    /// GET request, returns JSON deserialized to R
    pub async fn fetch<R>(&self, url: &str) -> Response<R>
    where
        R: DeserializeOwned,
    {
        self.get(url).send_json().await
    }

    /// POST with JSON body, returns JSON deserialized to R
    pub async fn post_json<B, R>(&self, url: &str, body: &B) -> Response<R>
    where
        B: Serialize + ?Sized,
        R: DeserializeOwned,
    {
        self.post(url).json(body).send_json().await
    }

    /// POST with form data, returns JSON deserialized to R
    pub async fn post_form<F, R>(&self, url: &str, form: &F) -> Response<R>
    where
        F: Serialize + ?Sized,
        R: DeserializeOwned,
    {
        self.post(url).form(form).send_json().await
    }

    /// PATCH with JSON body, returns JSON deserialized to R
    pub async fn patch_json<B, R>(&self, url: &str, body: &B) -> Response<R>
    where
        B: Serialize + ?Sized,
        R: DeserializeOwned,
    {
        self.patch(url).json(body).send_json().await
    }

    // === Raw request methods ===

    /// GET request returning raw response body
    pub async fn get_raw(&self, url: &str) -> Response<RawResponse> {
        self.get(url).send().await
    }

    // === Request builder methods ===

    /// POST request builder for complex cases (custom headers, form data, etc.)
    pub fn post(&self, url: &str) -> RequestBuilder {
        RequestBuilder::new("POST", url)
    }

    /// GET request builder for complex cases (custom headers, etc.)
    pub fn get(&self, url: &str) -> RequestBuilder {
        RequestBuilder::new("GET", url)
    }

    /// PATCH request builder for complex cases (custom headers, JSON body, etc.)
    pub fn patch(&self, url: &str) -> RequestBuilder {
        RequestBuilder::new("PATCH", url)
    }
}

/// HTTP client builder for configuring proxy and TLS settings
#[derive(Debug, Default)]
pub struct HttpClientBuilder;

impl HttpClientBuilder {
    /// Build the HTTP client
    pub fn build(self) -> Response<HttpClient> {
        Ok(HttpClient::new())
    }
}

/// Convenience function for simple GET requests
pub async fn fetch<R: DeserializeOwned>(url: &str) -> Response<R> {
    HttpClient::new().fetch(url).await
}

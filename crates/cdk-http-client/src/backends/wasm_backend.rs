//! WASM fetch-based backend implementation

use serde::de::DeserializeOwned;
use serde::Serialize;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

use crate::error::HttpError;
use crate::response::{RawResponse, Response};

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_name = "fetch")]
    fn js_fetch(input: &web_sys::Request) -> js_sys::Promise;
}

/// HTTP client wrapper
#[derive(Clone, Debug)]
pub struct HttpClient {
    no_redirects: bool,
}

impl HttpClient {
    /// Create a new HTTP client with default settings
    pub fn new() -> Self {
        Self {
            no_redirects: false,
        }
    }

    /// Create a new HTTP client builder
    pub fn builder() -> HttpClientBuilder {
        HttpClientBuilder::default()
    }

    /// GET request, returns JSON deserialized to R
    pub async fn fetch<R: DeserializeOwned>(&self, url: &str) -> Response<R> {
        self.get(url).send_json().await
    }

    /// POST with JSON body, returns JSON deserialized to R
    pub async fn post_json<B: Serialize, R: DeserializeOwned>(
        &self,
        url: &str,
        body: &B,
    ) -> Response<R> {
        self.post(url).json(body).send_json().await
    }

    /// POST with form data, returns JSON deserialized to R
    pub async fn post_form<F: Serialize, R: DeserializeOwned>(
        &self,
        url: &str,
        form: &F,
    ) -> Response<R> {
        self.post(url).form(form).send_json().await
    }

    /// PATCH with JSON body, returns JSON deserialized to R
    pub async fn patch_json<B: Serialize, R: DeserializeOwned>(
        &self,
        url: &str,
        body: &B,
    ) -> Response<R> {
        self.patch(url).json(body).send_json().await
    }

    /// GET request returning raw response body
    pub async fn get_raw(&self, url: &str) -> Response<RawResponse> {
        self.get(url).send().await
    }

    /// POST request builder for complex cases
    pub fn post(&self, url: &str) -> WasmRequestBuilder {
        WasmRequestBuilder::new("POST", url, self.no_redirects)
    }

    /// GET request builder for complex cases
    pub fn get(&self, url: &str) -> WasmRequestBuilder {
        WasmRequestBuilder::new("GET", url, self.no_redirects)
    }

    /// PATCH request builder for complex cases
    pub fn patch(&self, url: &str) -> WasmRequestBuilder {
        WasmRequestBuilder::new("PATCH", url, self.no_redirects)
    }
}

/// WASM fetch-based RequestBuilder wrapper
#[derive(Debug)]
pub struct WasmRequestBuilder {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: Option<WasmBody>,
    error: Option<HttpError>,
    no_redirects: bool,
}

#[derive(Debug)]
enum WasmBody {
    Json(Vec<u8>),
    Form(String),
}

impl WasmRequestBuilder {
    /// Create a new WasmRequestBuilder
    pub(crate) fn new(method: &str, url: &str, no_redirects: bool) -> Self {
        Self {
            method: method.to_string(),
            url: url.to_string(),
            headers: Vec::new(),
            body: None,
            error: None,
            no_redirects,
        }
    }

    async fn execute(self) -> Response<RawResponse> {
        if let Some(err) = self.error {
            return Err(err);
        }
        let opts = web_sys::RequestInit::new();
        opts.set_method(&self.method);
        if self.no_redirects {
            opts.set_redirect(web_sys::RequestRedirect::Error);
        }

        if let Some(body) = &self.body {
            match body {
                WasmBody::Json(bytes) => {
                    let uint8_array = js_sys::Uint8Array::from(bytes.as_slice());
                    opts.set_body(&uint8_array.into());
                }
                WasmBody::Form(form_str) => {
                    opts.set_body(&JsValue::from_str(form_str));
                }
            }
        }

        let request = web_sys::Request::new_with_str_and_init(&self.url, &opts)
            .map_err(|e| HttpError::Other(format!("Failed to create request: {:?}", e)))?;

        let headers = request.headers();
        for (key, value) in &self.headers {
            headers
                .set(key, value)
                .map_err(|e| HttpError::Other(format!("Failed to set header: {:?}", e)))?;
        }

        let resp_value = JsFuture::from(js_fetch(&request))
            .await
            .map_err(|e| HttpError::Connection(format!("Fetch failed: {:?}", e)))?;

        let resp: web_sys::Response = resp_value
            .dyn_into()
            .map_err(|_| HttpError::Other("Response is not a web_sys::Response".to_string()))?;

        let status = resp.status();

        let body_promise = resp
            .array_buffer()
            .map_err(|e| HttpError::Other(format!("Failed to read body: {:?}", e)))?;

        let body_value = JsFuture::from(body_promise)
            .await
            .map_err(|e| HttpError::Other(format!("Failed to read body: {:?}", e)))?;

        let body_array = js_sys::Uint8Array::new(&body_value);
        let body = body_array.to_vec();

        Ok(RawResponse::new(status, body))
    }
    /// Add a header to the request.
    pub fn header(mut self, key: impl AsRef<str>, value: impl AsRef<str>) -> Self {
        self.headers
            .push((key.as_ref().to_string(), value.as_ref().to_string()));
        self
    }

    /// Set the request body as JSON.
    pub fn json<T>(mut self, body: &T) -> Self
    where
        T: Serialize + ?Sized,
    {
        match serde_json::to_vec(body) {
            Ok(bytes) => {
                self.body = Some(WasmBody::Json(bytes));
                self.headers
                    .push(("Content-Type".to_string(), "application/json".to_string()));
            }
            Err(e) => self.error = Some(HttpError::Serialization(e.to_string())),
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
                self.body = Some(WasmBody::Form(form_str));
                self.headers.push((
                    "Content-Type".to_string(),
                    "application/x-www-form-urlencoded".to_string(),
                ));
            }
            Err(e) => self.error = Some(HttpError::Serialization(e.to_string())),
        }
        self
    }

    /// Send the request and return a raw response.
    pub async fn send(self) -> Response<RawResponse> {
        self.execute().await
    }

    /// Send the request and deserialize the response as JSON.
    pub async fn send_json<R: DeserializeOwned>(self) -> Response<R> {
        self.execute().await?.json_or_status_error()
    }
}

/// HTTP client builder for configuring proxy and TLS settings
#[derive(Debug, Default)]
pub struct HttpClientBuilder {
    no_redirects: bool,
    error: Option<HttpError>,
}

impl HttpClientBuilder {
    /// Accept invalid TLS certificates (not supported on WASM)
    pub fn danger_accept_invalid_certs(mut self, accept: bool) -> Self {
        if accept {
            self.set_error(HttpError::Build(
                "danger_accept_invalid_certs configuration is not supported on WASM".to_string(),
            ));
        }
        self
    }

    /// Disable automatic HTTP redirect following.
    pub fn no_redirects(mut self) -> Self {
        self.no_redirects = true;
        self
    }

    /// Set a proxy URL (not supported on WASM)
    pub fn proxy(mut self, _url: url::Url) -> Self {
        self.set_proxy_error();
        self
    }

    /// Set a proxy URL with a host pattern matcher (not supported on WASM)
    pub fn proxy_with_matcher(mut self, _url: url::Url, pattern: &str) -> Response<Self> {
        regex::Regex::new(pattern)
            .map_err(|e| HttpError::Proxy(format!("Invalid proxy pattern: {}", e)))?;
        self.set_proxy_error();
        Ok(self)
    }

    /// Build the HTTP client
    pub fn build(self) -> Response<HttpClient> {
        if let Some(err) = self.error {
            return Err(err);
        }

        Ok(HttpClient {
            no_redirects: self.no_redirects,
        })
    }

    fn set_proxy_error(&mut self) {
        self.set_error(HttpError::Proxy(
            "proxy configuration is not supported on WASM".to_string(),
        ));
    }

    fn set_error(&mut self, error: HttpError) {
        if self.error.is_none() {
            self.error = Some(error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_invalid_certs_false_is_noop() {
        let result = HttpClientBuilder::default()
            .danger_accept_invalid_certs(false)
            .build();

        assert!(result.is_ok());
    }

    #[test]
    fn accept_invalid_certs_true_returns_build_error() {
        let result = HttpClientBuilder::default()
            .danger_accept_invalid_certs(true)
            .build();

        match result {
            Err(HttpError::Build(message)) => {
                assert!(message.contains("danger_accept_invalid_certs"));
            }
            _ => panic!("Expected HttpError::Build"),
        }
    }

    #[test]
    fn proxy_returns_proxy_error_from_build() {
        let proxy_url = url::Url::parse("http://localhost:8080").expect("Valid proxy URL");
        let result = HttpClientBuilder::default().proxy(proxy_url).build();

        match result {
            Err(HttpError::Proxy(message)) => {
                assert!(message.contains("proxy configuration"));
            }
            _ => panic!("Expected HttpError::Proxy"),
        }
    }

    #[test]
    fn proxy_with_matcher_preserves_invalid_pattern_error() {
        let proxy_url = url::Url::parse("http://localhost:8080").expect("Valid proxy URL");
        let result = HttpClientBuilder::default().proxy_with_matcher(proxy_url, "[invalid");

        match result {
            Err(HttpError::Proxy(message)) => {
                assert!(message.contains("Invalid proxy pattern"));
            }
            _ => panic!("Expected HttpError::Proxy"),
        }
    }
}

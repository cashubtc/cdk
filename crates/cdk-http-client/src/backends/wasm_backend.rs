//! WASM fetch-based backend implementation

use serde::de::DeserializeOwned;
use serde::Serialize;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

use crate::error::HttpError;
use crate::request_builder_ext::RequestBuilderExt;
use crate::response::{RawResponse, Response};

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_name = "fetch")]
    fn js_fetch(input: &web_sys::Request) -> js_sys::Promise;
}

/// HTTP client wrapper
#[derive(Clone, Debug)]
pub struct HttpClient;

impl HttpClient {
    /// Create a new HTTP client with default settings
    pub fn new() -> Self {
        Self
    }

    /// Create a new HTTP client builder
    pub fn builder() -> HttpClientBuilder {
        HttpClientBuilder::default()
    }

    /// GET request, returns JSON deserialized to R
    pub async fn fetch<R: DeserializeOwned>(&self, url: &str) -> Response<R> {
        WasmRequestBuilder::new("GET", url).send_json().await
    }

    /// POST with JSON body, returns JSON deserialized to R
    pub async fn post_json<B: Serialize, R: DeserializeOwned>(
        &self,
        url: &str,
        body: &B,
    ) -> Response<R> {
        WasmRequestBuilder::new("POST", url)
            .json(body)
            .send_json()
            .await
    }

    /// POST with form data, returns JSON deserialized to R
    pub async fn post_form<F: Serialize, R: DeserializeOwned>(
        &self,
        url: &str,
        form: &F,
    ) -> Response<R> {
        WasmRequestBuilder::new("POST", url)
            .form(form)
            .send_json()
            .await
    }

    /// PATCH with JSON body, returns JSON deserialized to R
    pub async fn patch_json<B: Serialize, R: DeserializeOwned>(
        &self,
        url: &str,
        body: &B,
    ) -> Response<R> {
        WasmRequestBuilder::new("PATCH", url)
            .json(body)
            .send_json()
            .await
    }

    /// GET request returning raw response body
    pub async fn get_raw(&self, url: &str) -> Response<RawResponse> {
        WasmRequestBuilder::new("GET", url).send().await
    }

    /// POST request builder for complex cases
    pub fn post(&self, url: &str) -> WasmRequestBuilder {
        WasmRequestBuilder::new("POST", url)
    }

    /// GET request builder for complex cases
    pub fn get(&self, url: &str) -> WasmRequestBuilder {
        WasmRequestBuilder::new("GET", url)
    }

    /// PATCH request builder for complex cases
    pub fn patch(&self, url: &str) -> WasmRequestBuilder {
        WasmRequestBuilder::new("PATCH", url)
    }
}

/// WASM fetch-based RequestBuilder wrapper
#[derive(Debug)]
pub struct WasmRequestBuilder {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: Option<WasmBody>,
}

#[derive(Debug)]
enum WasmBody {
    Json(Vec<u8>),
    Form(String),
}

impl WasmRequestBuilder {
    /// Create a new WasmRequestBuilder
    pub(crate) fn new(method: &str, url: &str) -> Self {
        Self {
            method: method.to_string(),
            url: url.to_string(),
            headers: Vec::new(),
            body: None,
        }
    }

    async fn execute(self) -> Response<RawResponse> {
        let opts = web_sys::RequestInit::new();
        opts.set_method(&self.method);

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
}

impl RequestBuilderExt for WasmRequestBuilder {
    fn header(mut self, key: impl AsRef<str>, value: impl AsRef<str>) -> Self {
        self.headers
            .push((key.as_ref().to_string(), value.as_ref().to_string()));
        self
    }

    fn json<T: Serialize>(mut self, body: &T) -> Self {
        match serde_json::to_vec(body) {
            Ok(bytes) => {
                self.body = Some(WasmBody::Json(bytes));
                self.headers
                    .push(("Content-Type".to_string(), "application/json".to_string()));
            }
            Err(_) => {} // Error will surface when trying to send
        }
        self
    }

    fn form<T: Serialize>(mut self, body: &T) -> Self {
        match serde_urlencoded::to_string(body) {
            Ok(form_str) => {
                self.body = Some(WasmBody::Form(form_str));
                self.headers.push((
                    "Content-Type".to_string(),
                    "application/x-www-form-urlencoded".to_string(),
                ));
            }
            Err(_) => {}
        }
        self
    }

    async fn send(self) -> Response<RawResponse> {
        self.execute().await
    }

    async fn send_json<R: DeserializeOwned>(self) -> Response<R> {
        let raw = self.execute().await?;
        let status = raw.status();

        if !raw.is_success() {
            let message = String::from_utf8_lossy(&raw.body).to_string();
            return Err(HttpError::Status { status, message });
        }

        serde_json::from_slice(&raw.body).map_err(HttpError::from)
    }
}

/// HTTP client builder for configuring proxy and TLS settings
#[derive(Debug, Default)]
pub struct HttpClientBuilder;

impl HttpClientBuilder {
    /// Accept invalid TLS certificates (not supported on WASM)
    pub fn danger_accept_invalid_certs(self, _accept: bool) -> Self {
        panic!("danger_accept_invalid_certs configuration is not supported on WASM")
    }

    /// Set a proxy URL (not supported on WASM)
    pub fn proxy(self, _url: url::Url) -> Self {
        panic!("proxy configuration is not supported on WASM")
    }

    /// Set a proxy URL with a host pattern matcher (not supported on WASM)
    pub fn proxy_with_matcher(self, _url: url::Url, _pattern: &str) -> Response<Self> {
        panic!("proxy configuration is not supported on WASM")
    }

    /// Build the HTTP client
    pub fn build(self) -> Response<HttpClient> {
        Ok(HttpClient)
    }
}

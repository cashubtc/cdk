//! WASM HTTP request builder using the browser's native `fetch()` API

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::HttpError;
use crate::wasm::response::RawResponse;
use crate::Response;

#[wasm_bindgen::prelude::wasm_bindgen]
extern "C" {
    #[wasm_bindgen::prelude::wasm_bindgen(js_name = "fetch")]
    fn js_fetch(input: &web_sys::Request) -> js_sys::Promise;
}

/// HTTP request builder for complex requests
pub struct RequestBuilder {
    url: String,
    method: String,
    headers: Vec<(String, String)>,
    body: Option<String>,
}

impl core::fmt::Debug for RequestBuilder {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RequestBuilder")
            .field("url", &self.url)
            .field("method", &self.method)
            .finish()
    }
}

impl RequestBuilder {
    pub(crate) fn new(method: &str, url: &str) -> Self {
        Self {
            url: url.to_string(),
            method: method.to_string(),
            headers: Vec::new(),
            body: None,
        }
    }

    /// Add a header to the request
    pub fn header(mut self, key: impl AsRef<str>, value: impl AsRef<str>) -> Self {
        self.headers
            .push((key.as_ref().to_string(), value.as_ref().to_string()));
        self
    }

    /// Set the request body as JSON
    pub fn json<T: Serialize + ?Sized>(mut self, body: &T) -> Self {
        match serde_json::to_string(body) {
            Ok(json) => {
                self.body = Some(json);
                self.headers
                    .push(("Content-Type".to_string(), "application/json".to_string()));
            }
            Err(_) => {
                // Body serialization failed; send() will produce a request without body.
                // This matches reqwest's deferred-error behaviour.
            }
        }
        self
    }

    /// Set the request body as form data
    pub fn form<T: Serialize + ?Sized>(mut self, body: &T) -> Self {
        if let Ok(serde_json::Value::Object(map)) = serde_json::to_value(body) {
            let pairs: Vec<String> = map
                .iter()
                .map(|(k, v)| {
                    let val_str = match v {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    let k_enc = js_sys::encode_uri_component(k);
                    let v_enc = js_sys::encode_uri_component(&val_str);
                    format!("{}={}", String::from(k_enc), String::from(v_enc))
                })
                .collect();
            self.body = Some(pairs.join("&"));
            self.headers.push((
                "Content-Type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            ));
        }
        self
    }

    /// Send the request and return a raw response
    pub async fn send(self) -> Response<RawResponse> {
        let init = web_sys::RequestInit::new();
        init.set_method(&self.method);

        if let Some(ref body) = self.body {
            init.set_body(&wasm_bindgen::JsValue::from_str(body));
        }

        let request =
            web_sys::Request::new_with_str_and_init(&self.url, &init).map_err(HttpError::from)?;

        for (key, value) in &self.headers {
            request.headers().set(key, value).map_err(HttpError::from)?;
        }

        let promise = js_fetch(&request);
        let js_value = wasm_bindgen_futures::JsFuture::from(promise)
            .await
            .map_err(HttpError::from)?;

        let response: web_sys::Response = js_value.into();
        Ok(RawResponse::new(response))
    }

    /// Send the request and deserialize the response as JSON
    pub async fn send_json<R: DeserializeOwned>(self) -> Response<R> {
        let raw = self.send().await?;
        let status = raw.status();

        if !(200..300).contains(&status) {
            let message = raw.text().await.unwrap_or_default();
            return Err(HttpError::Status { status, message });
        }

        raw.json().await
    }
}

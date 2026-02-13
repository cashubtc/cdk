//! WASM implementation using the browser's native `fetch()` API

mod client;
mod request;
mod response;

pub use client::{fetch, HttpClient, HttpClientBuilder};
pub use request::RequestBuilder;
pub use response::RawResponse;

use crate::error::HttpError;

impl From<wasm_bindgen::JsValue> for HttpError {
    fn from(err: wasm_bindgen::JsValue) -> Self {
        let message = err
            .as_string()
            .or_else(|| {
                js_sys::Reflect::get(&err, &"message".into())
                    .ok()
                    .and_then(|v| v.as_string())
            })
            .unwrap_or_else(|| format!("{:?}", err));
        HttpError::Other(message)
    }
}

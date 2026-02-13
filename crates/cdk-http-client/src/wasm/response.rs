//! WASM HTTP response wrapping `web_sys::Response`

use serde::de::DeserializeOwned;

use crate::error::HttpError;
use crate::Response;

/// Raw HTTP response with status code and body access
pub struct RawResponse {
    status: u16,
    inner: web_sys::Response,
}

impl core::fmt::Debug for RawResponse {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RawResponse")
            .field("status", &self.status)
            .finish()
    }
}

impl RawResponse {
    pub(crate) fn new(response: web_sys::Response) -> Self {
        let status = response.status();
        Self {
            status,
            inner: response,
        }
    }

    /// Get the HTTP status code
    pub fn status(&self) -> u16 {
        self.status
    }

    /// Check if the response status is a success (2xx)
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// Check if the response status is a client error (4xx)
    pub fn is_client_error(&self) -> bool {
        (400..500).contains(&self.status)
    }

    /// Check if the response status is a server error (5xx)
    pub fn is_server_error(&self) -> bool {
        (500..600).contains(&self.status)
    }

    /// Get the response body as text
    pub async fn text(self) -> Response<String> {
        let promise = self.inner.text().map_err(HttpError::from)?;
        let js_value = wasm_bindgen_futures::JsFuture::from(promise)
            .await
            .map_err(HttpError::from)?;
        js_value
            .as_string()
            .ok_or_else(|| HttpError::Other("Response body is not a string".into()))
    }

    /// Get the response body as JSON
    pub async fn json<T: DeserializeOwned>(self) -> Response<T> {
        let text = self.text().await?;
        serde_json::from_str(&text).map_err(HttpError::from)
    }

    /// Get the response body as bytes
    pub async fn bytes(self) -> Response<Vec<u8>> {
        let promise = self.inner.array_buffer().map_err(HttpError::from)?;
        let js_value = wasm_bindgen_futures::JsFuture::from(promise)
            .await
            .map_err(HttpError::from)?;
        let array = js_sys::Uint8Array::new(&js_value);
        Ok(array.to_vec())
    }
}

//! HTTP request builder

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::HttpError;
use crate::response::{RawResponse, Response};

/// HTTP request builder for complex requests
#[derive(Debug)]
pub struct RequestBuilder {
    #[cfg(target_arch = "wasm32")]
    inner: reqwest::RequestBuilder,
    #[cfg(not(target_arch = "wasm32"))]
    inner: bitreq::Request,
    #[cfg(not(target_arch = "wasm32"))]
    error: Option<HttpError>,
}

#[cfg(target_arch = "wasm32")]
impl RequestBuilder {
    /// Create a new RequestBuilder from a reqwest::RequestBuilder
    pub(crate) fn new(inner: reqwest::RequestBuilder) -> Self {
        Self { inner }
    }

    /// Add a header to the request
    pub fn header(self, key: impl AsRef<str>, value: impl AsRef<str>) -> Self {
        Self {
            inner: self.inner.header(key.as_ref(), value.as_ref()),
        }
    }

    /// Set the request body as JSON
    pub fn json<T: Serialize>(self, body: &T) -> Self {
        Self {
            inner: self.inner.json(body),
        }
    }

    /// Set the request body as form data
    pub fn form<T: Serialize>(self, body: &T) -> Self {
        Self {
            inner: self.inner.form(body),
        }
    }

    /// Send the request and return a raw response
    pub async fn send(self) -> Response<RawResponse> {
        let response = self.inner.send().await?;
        Ok(RawResponse::new(response))
    }

    /// Send the request and deserialize the response as JSON
    pub async fn send_json<R: DeserializeOwned>(self) -> Response<R> {
        let response = self.inner.send().await?;
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
}

#[cfg(not(target_arch = "wasm32"))]
impl RequestBuilder {
    /// Create a new RequestBuilder from a bitreq::Request
    pub(crate) fn new(inner: bitreq::Request) -> Self {
        Self {
            inner: inner,
            error: None,
        }
    }

    /// Add a header to the request
    pub fn header(self, key: impl AsRef<str>, value: impl AsRef<str>) -> Self {
        Self {
            inner: self.inner.with_header(key.as_ref(), value.as_ref()),
            error: None,
        }
    }

    /// Set the request body as JSON
    pub fn json<T: Serialize>(mut self, body: &T) -> Self {
        match self.inner.clone().with_json(body) {
            Ok(req) => {
                self.inner = req;
                self.error = None;
            }
            Err(e) => self.error = Some(HttpError::from(e)),
        }
        self
    }

    /// Set the request body as form data
    pub fn form<T: Serialize>(mut self, body: &T) -> Self {
        match serde_urlencoded::to_string(body) {
            Ok(form_str) => {
                self.inner = self.inner.with_body(form_str.into_bytes());
            }
            Err(e) => self.error = Some(HttpError::Serialization(e.to_string())),
        }
        self
    }

    /// Send the request and return a raw response
    pub async fn send(self) -> Response<RawResponse> {
        if let Some(err) = self.error {
            return Err(err);
        }
        let response = self.inner.send_async().await.map_err(HttpError::from)?;
        Ok(RawResponse::new(response))
    }

    /// Send the request and deserialize the response as JSON
    pub async fn send_json<R: DeserializeOwned>(self) -> Response<R> {
        if let Some(err) = self.error {
            return Err(err);
        }
        let response = self.inner.send_async().await.map_err(HttpError::from)?;
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

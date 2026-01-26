//! HTTP request builder

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::HttpError;
use crate::response::{RawResponse, Response};

/// HTTP request builder for complex requests
#[derive(Debug)]
pub struct RequestBuilder {
    inner: reqwest::RequestBuilder,
}

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
    pub fn json<T: Serialize + ?Sized>(self, body: &T) -> Self {
        Self {
            inner: self.inner.json(body),
        }
    }

    /// Set the request body as form data
    pub fn form<T: Serialize + ?Sized>(self, body: &T) -> Self {
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

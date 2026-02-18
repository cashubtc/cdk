//! reqwest-based RequestBuilder implementation

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::HttpError;
use crate::request_builder_ext::RequestBuilderExt;
use crate::response::{RawResponse, Response};

/// reqwest-based RequestBuilder wrapper
#[derive(Debug)]
pub struct ReqwestRequestBuilder {
    inner: reqwest::RequestBuilder,
}

impl ReqwestRequestBuilder {
    /// Create a new ReqwestRequestBuilder from a reqwest::RequestBuilder
    pub(crate) fn new(inner: reqwest::RequestBuilder) -> Self {
        Self { inner }
    }
}

impl RequestBuilderExt for ReqwestRequestBuilder {
    fn header(self, key: impl AsRef<str>, value: impl AsRef<str>) -> Self {
        Self {
            inner: self.inner.header(key.as_ref(), value.as_ref()),
        }
    }

    fn json<T: Serialize>(self, body: &T) -> Self {
        Self {
            inner: self.inner.json(body),
        }
    }

    fn form<T: Serialize>(self, body: &T) -> Self {
        Self {
            inner: self.inner.form(body),
        }
    }

    async fn send(self) -> Response<RawResponse> {
        let response = self.inner.send().await.map_err(HttpError::from)?;
        Ok(RawResponse::new(response))
    }

    async fn send_json<R: DeserializeOwned>(self) -> Response<R> {
        let response = self.inner.send().await.map_err(HttpError::from)?;
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

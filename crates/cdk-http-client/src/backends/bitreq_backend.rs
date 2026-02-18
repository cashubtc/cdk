//! bitreq-based RequestBuilder implementation

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::HttpError;
use crate::request_builder_ext::RequestBuilderExt;
use crate::response::{RawResponse, Response};

/// bitreq-based RequestBuilder wrapper
#[derive(Debug)]
pub struct BitreqRequestBuilder {
    inner: bitreq::Request,
    error: Option<HttpError>,
}

impl BitreqRequestBuilder {
    /// Create a new BitreqRequestBuilder from a bitreq::Request
    pub(crate) fn new(inner: bitreq::Request) -> Self {
        Self {
            inner,
            error: None,
        }
    }
}

impl RequestBuilderExt for BitreqRequestBuilder {
    fn header(self, key: impl AsRef<str>, value: impl AsRef<str>) -> Self {
        Self {
            inner: self.inner.with_header(key.as_ref(), value.as_ref()),
            error: None,
        }
    }

    fn json<T: Serialize>(mut self, body: &T) -> Self {
        match self.inner.clone().with_json(body) {
            Ok(req) => {
                self.inner = req;
                self.error = None;
            }
            Err(e) => self.error = Some(HttpError::from(e)),
        }
        self
    }

    fn form<T: Serialize>(mut self, body: &T) -> Self {
        match serde_urlencoded::to_string(body) {
            Ok(form_str) => {
                self.inner = self.inner.with_body(form_str.into_bytes());
            }
            Err(e) => self.error = Some(HttpError::Serialization(e.to_string())),
        }
        self
    }

    async fn send(self) -> Response<RawResponse> {
        if let Some(err) = self.error {
            return Err(err);
        }
        let response = self.inner.send_async().await.map_err(HttpError::from)?;
        Ok(RawResponse::new(response))
    }

    async fn send_json<R: DeserializeOwned>(self) -> Response<R> {
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

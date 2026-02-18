//! HTTP RequestBuilder extension trait

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::response::{RawResponse, Response};

/// Trait for building and sending HTTP requests
///
/// This trait abstracts over different HTTP client backends (bitreq, reqwest)
/// and provides a unified interface for building and sending HTTP requests.
pub trait RequestBuilderExt: Sized + Send {
    /// Add a header to the request
    fn header(self, key: impl AsRef<str>, value: impl AsRef<str>) -> Self;

    /// Set the request body as JSON
    fn json<T: Serialize>(self, body: &T) -> Self;

    /// Set the request body as form data
    fn form<T: Serialize>(self, body: &T) -> Self;

    /// Send the request and return a raw response
    fn send(self) -> impl std::future::Future<Output = Response<RawResponse>> + Send;

    /// Send the request and deserialize the response as JSON
    fn send_json<R: DeserializeOwned>(
        self,
    ) -> impl std::future::Future<Output = Response<R>> + Send;
}

#[allow(clippy::manual_async_fn)]
impl<T: RequestBuilderExt> RequestBuilderExt for Box<T> {
    fn header(self, key: impl AsRef<str>, value: impl AsRef<str>) -> Self {
        Box::new((*self).header(key, value))
    }

    fn json<B: Serialize>(self, body: &B) -> Self {
        Box::new((*self).json(body))
    }

    fn form<F: Serialize>(self, body: &F) -> Self {
        Box::new((*self).form(body))
    }

    fn send(self) -> impl std::future::Future<Output = Response<RawResponse>> + Send {
        async move { (*self).send().await }
    }

    fn send_json<R: DeserializeOwned>(
        self,
    ) -> impl std::future::Future<Output = Response<R>> + Send {
        async move { (*self).send_json().await }
    }
}

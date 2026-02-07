//! HTTP response types

use serde::de::DeserializeOwned;

use crate::error::HttpError;

/// HTTP Response type - generic over the body type R and error type E
/// This is the primary return type for all HTTP operations
pub type Response<R, E = HttpError> = Result<R, E>;

/// Raw HTTP response with status code and body access
#[derive(Debug)]
pub struct RawResponse {
    status: u16,
    #[cfg(target_arch = "wasm32")]
    inner: reqwest::Response,
    #[cfg(not(target_arch = "wasm32"))]
    inner: bitreq::Response,
}

#[cfg(target_arch = "wasm32")]
impl RawResponse {
    /// Create a new RawResponse from a reqwest::Response
    pub(crate) fn new(response: reqwest::Response) -> Self {
        Self {
            status: response.status().as_u16(),
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
        self.inner.text().await.map_err(HttpError::from)
    }

    /// Get the response body as JSON
    pub async fn json<T: DeserializeOwned>(self) -> Response<T> {
        self.inner.json().await.map_err(HttpError::from)
    }

    /// Get the response body as bytes
    pub async fn bytes(self) -> Response<Vec<u8>> {
        self.inner
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(HttpError::from)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl RawResponse {
    /// Create a new RawResponse from a bitreq::Response
    pub(crate) fn new(response: bitreq::Response) -> Self {
        Self {
            status: response.status_code as u16,
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
        self.inner
            .as_str()
            .map(|s| s.to_string())
            .map_err(HttpError::from)
    }

    /// Get the response body as JSON
    pub async fn json<T: DeserializeOwned>(self) -> Response<T> {
        self.inner.json().map_err(HttpError::from)
    }

    /// Get the response body as bytes
    pub async fn bytes(self) -> Response<Vec<u8>> {
        Ok(self.inner.into_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: RawResponse tests require a real response,
    // so they are in tests/integration.rs using mockito.

    #[test]
    fn test_response_type_is_result() {
        // Response<R, E> is just a type alias for Result<R, E>
        let success: Response<i32> = Ok(42);
        assert!(success.is_ok());
        assert!(matches!(success, Ok(42)));

        let error: Response<i32> = Err(HttpError::Timeout);
        assert!(error.is_err());
        assert!(matches!(error, Err(HttpError::Timeout)));
    }
}

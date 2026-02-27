//! HTTP client wrapper

use serde::de::DeserializeOwned;

pub use crate::backends::{HttpClient, HttpClientBuilder};
use crate::response::Response;

impl Default for HttpClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Convenience function for simple GET requests
pub async fn fetch<R: DeserializeOwned>(url: &str) -> Response<R> {
    HttpClient::new().fetch(url).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_new() {
        let client = HttpClient::new();
        let _ = format!("{:?}", client);
    }

    #[test]
    fn test_client_default() {
        let client = HttpClient::default();
        let _ = format!("{:?}", client);
    }

    #[test]
    fn test_builder_returns_builder() {
        let builder = HttpClient::builder();
        let _ = format!("{:?}", builder);
    }

    #[test]
    fn test_builder_build() {
        let result = HttpClientBuilder::default().build();
        assert!(result.is_ok());
    }

    #[cfg(not(target_arch = "wasm32"))]
    mod bitreq_tests {
        use super::*;
        use crate::error::HttpError;

        #[test]
        fn test_builder_accept_invalid_certs_returns_error() {
            let result = HttpClientBuilder::default()
                .danger_accept_invalid_certs(true)
                .build();
            assert!(result.is_err());
            if let Err(HttpError::Build(msg)) = result {
                assert!(msg.contains("danger_accept_invalid_certs"));
            } else {
                panic!("Expected HttpError::Build");
            }
        }

        #[test]
        fn test_builder_accept_invalid_certs_false_ok() {
            let result = HttpClientBuilder::default()
                .danger_accept_invalid_certs(false)
                .build();
            assert!(result.is_ok());
        }

        #[test]
        fn test_builder_proxy() {
            let proxy_url = url::Url::parse("http://localhost:8080").expect("Valid proxy URL");
            let result = HttpClientBuilder::default().proxy(proxy_url).build();
            assert!(result.is_ok());
        }

        #[test]
        fn test_builder_proxy_with_valid_matcher() {
            let proxy_url = url::Url::parse("http://localhost:8080").expect("Valid proxy URL");
            let result =
                HttpClientBuilder::default().proxy_with_matcher(proxy_url, r".*\.example\.com$");
            assert!(result.is_ok());

            let builder = result.expect("Valid matcher should succeed");
            let client_result = builder.build();
            assert!(client_result.is_ok());
        }

        #[test]
        fn test_builder_proxy_with_invalid_matcher() {
            let proxy_url = url::Url::parse("http://localhost:8080").expect("Valid proxy URL");
            let result = HttpClientBuilder::default().proxy_with_matcher(proxy_url, r"[invalid");
            assert!(result.is_err());

            if let Err(HttpError::Proxy(msg)) = result {
                assert!(msg.contains("Invalid proxy pattern"));
            } else {
                panic!("Expected HttpError::Proxy");
            }
        }
    }
}

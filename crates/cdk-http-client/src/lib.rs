//! HTTP client abstraction for CDK
//!
//! This crate provides an HTTP client wrapper that abstracts the underlying HTTP library.
//! Using this crate allows other CDK crates to avoid direct dependencies on a specific backend.
//! Backend selection is feature-based: enable either `bitreq` (default) or `reqwest`.
//!
//! # Example
//!
//! ```no_run
//! use cdk_http_client::{HttpClient, Response};
//! use serde::Deserialize;
//!
//! #[derive(Deserialize)]
//! struct ApiResponse {
//!     message: String,
//! }
//!
//! async fn example() -> Response<ApiResponse> {
//!     let client = HttpClient::new();
//!     client.fetch("https://api.example.com/data").await
//! }
//! ```

mod backends;
mod client;
mod error;
mod request;
mod request_builder_ext;
mod response;
pub mod ws;
mod transport;

#[cfg(feature = "bitreq")]
pub use backends::BitreqRequestBuilder;
#[cfg(feature = "reqwest")]
pub use backends::ReqwestRequestBuilder;
pub use client::{fetch, HttpClient, HttpClientBuilder};
pub use error::HttpError;
pub use request::{RequestBuilder, RequestBuilderExt};
pub use response::{RawResponse, Response};
#[cfg(any(feature = "bitreq", feature = "reqwest"))]
pub use transport::Async;
#[cfg(feature = "bitreq")]
pub use transport::BitreqTransport;
#[cfg(feature = "reqwest")]
pub use transport::ReqwestTransport;
#[cfg(feature = "tor")]
pub use transport::TorAsync;
pub use transport::Transport;

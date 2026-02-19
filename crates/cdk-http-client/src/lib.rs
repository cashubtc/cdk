//! HTTP client abstraction for CDK
//!
//! This crate provides an HTTP client wrapper that abstracts the underlying HTTP library
//! (bitreq on native, fetch API on WASM).
//! Using this crate allows other CDK crates to avoid direct dependencies on a specific backend.
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

#[cfg(not(target_arch = "wasm32"))]
pub use backends::BitreqRequestBuilder;
#[cfg(target_arch = "wasm32")]
pub use backends::WasmRequestBuilder;
pub use client::{fetch, HttpClient, HttpClientBuilder};
pub use error::HttpError;
pub use request::{RequestBuilder, RequestBuilderExt};
pub use response::{RawResponse, Response};

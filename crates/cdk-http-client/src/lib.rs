//! HTTP client abstraction for CDK
//!
//! This crate provides an HTTP client wrapper that abstracts the underlying HTTP library.
//! On native targets it uses reqwest; on WASM it calls the browser's `fetch()` API directly.
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

mod error;

#[cfg(not(target_arch = "wasm32"))]
mod client;
#[cfg(not(target_arch = "wasm32"))]
mod request;
#[cfg(not(target_arch = "wasm32"))]
mod response;

#[cfg(target_arch = "wasm32")]
mod wasm;

// Shared
// Native re-exports
#[cfg(not(target_arch = "wasm32"))]
pub use client::{fetch, HttpClient, HttpClientBuilder};
pub use error::{HttpError, Response};
#[cfg(not(target_arch = "wasm32"))]
pub use request::RequestBuilder;
#[cfg(not(target_arch = "wasm32"))]
pub use response::RawResponse;
// WASM re-exports
#[cfg(target_arch = "wasm32")]
pub use wasm::{fetch, HttpClient, HttpClientBuilder, RawResponse, RequestBuilder};

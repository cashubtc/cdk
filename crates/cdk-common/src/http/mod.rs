//! HTTP client abstraction
//!
//! This module provides an HTTP client wrapper that abstracts the underlying HTTP library (reqwest).
//! Using this module allows other crates to avoid direct dependencies on reqwest.
//!
//! # Example
//!
//! ```no_run
//! use cdk_common::http::{HttpClient, Response};
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

mod client;
mod error;
mod request;
mod response;

pub use client::{fetch, HttpClient, HttpClientBuilder};
pub use error::HttpError;
pub use request::RequestBuilder;
pub use response::{RawResponse, Response};

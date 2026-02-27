//! HTTP client abstraction for CDK
//!
//! This crate provides an HTTP client wrapper that abstracts the underlying HTTP library (reqwest).
//! Using this crate allows other CDK crates to avoid direct dependencies on reqwest.
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

mod client;
mod error;
mod request;
mod response;
pub mod ws;

pub use client::{fetch, HttpClient, HttpClientBuilder};
pub use error::HttpError;
pub use request::RequestBuilder;
pub use response::{RawResponse, Response};

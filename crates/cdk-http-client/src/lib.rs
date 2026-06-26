//! HTTP client abstraction for CDK
//!
//! This crate provides an HTTP client wrapper that abstracts the underlying HTTP library.
//! Using this crate allows other CDK crates to avoid direct dependencies on a specific backend.
//! Backend selection is feature-based: enable a native backend with `bitreq`
//! (default) or `reqwest`. The features are additive — when both are enabled,
//! `reqwest` takes precedence (see "Backend selection" below).
//! The default `bitreq` backend supports HTTP proxy URLs only. SOCKS proxy schemes
//! such as `socks5h` are supported only when this crate is built with the `reqwest`
//! feature.
//!
//! # Backend selection
//!
//! CDK library crates depend on HTTP capability, while this crate owns the concrete
//! backend choice. The `bitreq` backend is the default: any CDK crate that pulls in
//! HTTP support turns it on automatically. `cdk-common/http` enables `bitreq`, and
//! `cdk`'s `wallet` and `mint` features depend on it, so applications get a working
//! client out of the box — including with `cdk --no-default-features --features wallet`.
//!
//! The backend features are additive: enabling both `bitreq` and `reqwest`
//! resolves to `reqwest` (a strict superset that adds SOCKS proxy and
//! invalid-certificate support), so feature unification across a dependency graph
//! never conflicts. To use `reqwest`, add an explicit dependency on this crate with
//! the `reqwest` feature — it takes precedence wherever it is enabled:
//!
//! ```toml
//! [dependencies]
//! cdk = { version = "0.17.0", features = ["wallet"] }
//! cdk-http-client = { version = "0.17.0", features = ["reqwest"] }
//! ```
//!
//! When depending on this crate directly with `--no-default-features`, at least one
//! backend must be selected, or compilation fails with a clear error. To exercise the
//! `reqwest` backend in workspace checks:
//!
//! ```bash
//! cargo check -p cdk -p cdk-http-client --features cdk-http-client/reqwest
//! ```
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
mod response;
mod transport;
pub mod ws;

#[cfg(all(
    feature = "bitreq",
    not(feature = "reqwest"),
    not(target_arch = "wasm32")
))]
pub use backends::BitreqRequestBuilder;
#[cfg(all(feature = "reqwest", not(target_arch = "wasm32")))]
pub use backends::ReqwestRequestBuilder;
#[cfg(target_arch = "wasm32")]
pub use backends::WasmRequestBuilder;
pub use client::{fetch, HttpClient, HttpClientBuilder};
pub use error::HttpError;
pub use request::RequestBuilder;
pub use response::{RawResponse, Response};
#[cfg(any(target_arch = "wasm32", feature = "bitreq", feature = "reqwest"))]
pub use transport::Async;
#[cfg(all(
    feature = "bitreq",
    not(feature = "reqwest"),
    not(target_arch = "wasm32")
))]
pub use transport::BitreqTransport;
#[cfg(all(feature = "reqwest", not(target_arch = "wasm32")))]
pub use transport::ReqwestTransport;
#[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
pub use transport::TorAsync;
pub use transport::Transport;

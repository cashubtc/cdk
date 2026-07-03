//! HTTP request builder backends
//!
//! Backend selection is additive: `reqwest` takes precedence when both the
//! `reqwest` and `bitreq` features are enabled. `reqwest` is a strict superset
//! (it adds SOCKS proxy and invalid-certificate support), so enabling it only
//! ever adds capability. `bitreq` is used only when `reqwest` is off. Keeping
//! the features additive means Cargo feature unification across a dependency
//! graph can never produce a build conflict.

#[cfg(all(
    feature = "bitreq",
    not(feature = "reqwest"),
    not(target_arch = "wasm32")
))]
pub mod bitreq_backend;

#[cfg(all(feature = "reqwest", not(target_arch = "wasm32")))]
pub mod reqwest_backend;

#[cfg(target_arch = "wasm32")]
pub mod wasm_backend;

#[cfg(all(
    not(target_arch = "wasm32"),
    not(any(feature = "bitreq", feature = "reqwest"))
))]
compile_error!("Enable either the `bitreq` or `reqwest` feature for cdk-http-client.");

#[cfg(all(
    feature = "bitreq",
    not(feature = "reqwest"),
    not(target_arch = "wasm32")
))]
pub use bitreq_backend::BitreqRequestBuilder as RequestBuilder;
#[cfg(all(
    feature = "bitreq",
    not(feature = "reqwest"),
    not(target_arch = "wasm32")
))]
pub use bitreq_backend::BitreqRequestBuilder;
#[cfg(all(
    feature = "bitreq",
    not(feature = "reqwest"),
    not(target_arch = "wasm32")
))]
pub use bitreq_backend::{HttpClient, HttpClientBuilder};
#[cfg(all(feature = "reqwest", not(target_arch = "wasm32")))]
pub use reqwest_backend::ReqwestRequestBuilder as RequestBuilder;
#[cfg(all(feature = "reqwest", not(target_arch = "wasm32")))]
pub use reqwest_backend::{HttpClient, HttpClientBuilder, ReqwestRequestBuilder};
#[cfg(target_arch = "wasm32")]
pub use wasm_backend::WasmRequestBuilder as RequestBuilder;
#[cfg(target_arch = "wasm32")]
pub use wasm_backend::{HttpClient, HttpClientBuilder, WasmRequestBuilder};

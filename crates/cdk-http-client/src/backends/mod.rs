//! HTTP request builder backends

#[cfg(all(feature = "bitreq", not(target_arch = "wasm32")))]
pub mod bitreq_backend;

#[cfg(all(feature = "reqwest", not(target_arch = "wasm32")))]
pub mod reqwest_backend;

#[cfg(target_arch = "wasm32")]
pub mod wasm_backend;

#[cfg(all(not(target_arch = "wasm32"), feature = "bitreq", feature = "reqwest"))]
compile_error!("`bitreq` and `reqwest` features are mutually exclusive for cdk-http-client.");

#[cfg(all(
    not(target_arch = "wasm32"),
    not(any(feature = "bitreq", feature = "reqwest"))
))]
compile_error!("Enable either the `bitreq` or `reqwest` feature for cdk-http-client.");

#[cfg(all(feature = "bitreq", not(target_arch = "wasm32")))]
pub use bitreq_backend::BitreqRequestBuilder as RequestBuilder;
#[cfg(all(feature = "bitreq", not(target_arch = "wasm32")))]
pub use bitreq_backend::{BitreqRequestBuilder, HttpClient, HttpClientBuilder};
#[cfg(all(feature = "reqwest", not(target_arch = "wasm32")))]
pub use reqwest_backend::ReqwestRequestBuilder as RequestBuilder;
#[cfg(all(feature = "reqwest", not(target_arch = "wasm32")))]
pub use reqwest_backend::{HttpClient, HttpClientBuilder, ReqwestRequestBuilder};
#[cfg(target_arch = "wasm32")]
pub use wasm_backend::WasmRequestBuilder as RequestBuilder;
#[cfg(target_arch = "wasm32")]
pub use wasm_backend::{HttpClient, HttpClientBuilder, WasmRequestBuilder};

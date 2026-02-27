//! HTTP request builder backends

#[cfg(not(target_arch = "wasm32"))]
pub mod bitreq_backend;

#[cfg(target_arch = "wasm32")]
pub mod wasm_backend;

#[cfg(not(target_arch = "wasm32"))]
pub use bitreq_backend::{BitreqRequestBuilder, HttpClient, HttpClientBuilder};
#[cfg(target_arch = "wasm32")]
pub use wasm_backend::{HttpClient, HttpClientBuilder, WasmRequestBuilder};

//! HTTP request builder

/// HTTP request builder for complex requests
///
/// This is a type alias that resolves to either `BitreqRequestBuilder` or
/// `WasmRequestBuilder` depending on the target architecture.
#[cfg(not(target_arch = "wasm32"))]
pub use crate::backends::BitreqRequestBuilder as RequestBuilder;
#[cfg(target_arch = "wasm32")]
pub use crate::backends::WasmRequestBuilder as RequestBuilder;
pub use crate::request_builder_ext::RequestBuilderExt;

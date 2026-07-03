//! HTTP request builder

/// HTTP request builder for complex requests
///
/// This is a type alias that resolves to the selected native backend request
/// builder, or `WasmRequestBuilder` on wasm.
pub use crate::backends::RequestBuilder;

//! HTTP request builder

/// HTTP request builder for complex requests
///
/// This is a type alias that resolves to either `BitreqRequestBuilder` or
/// `ReqwestRequestBuilder` depending on enabled features.
pub use crate::backends::RequestBuilder;
pub use crate::request_builder_ext::RequestBuilderExt;

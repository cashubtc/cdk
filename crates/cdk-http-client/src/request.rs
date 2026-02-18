//! HTTP request builder

pub use crate::request_builder_ext::RequestBuilderExt;

/// HTTP request builder for complex requests
///
/// This is a type alias that resolves to either `BitreqRequestBuilder` or
/// `ReqwestRequestBuilder` depending on the enabled feature.
#[cfg(feature = "bitreq")]
pub use crate::backends::BitreqRequestBuilder as RequestBuilder;

#[cfg(feature = "reqwest")]
pub use crate::backends::ReqwestRequestBuilder as RequestBuilder;

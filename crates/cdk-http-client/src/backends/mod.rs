//! HTTP request builder backends

#[cfg(feature = "bitreq")]
pub mod bitreq_backend;

#[cfg(feature = "reqwest")]
pub mod reqwest_backend;

#[cfg(all(feature = "bitreq", feature = "reqwest"))]
compile_error!("`bitreq` and `reqwest` features are mutually exclusive for cdk-http-client.");

#[cfg(not(any(feature = "bitreq", feature = "reqwest")))]
compile_error!("Enable either the `bitreq` or `reqwest` feature for cdk-http-client.");

#[cfg(feature = "bitreq")]
pub use bitreq_backend::BitreqRequestBuilder as RequestBuilder;
#[cfg(feature = "bitreq")]
pub use bitreq_backend::{BitreqRequestBuilder, HttpClient, HttpClientBuilder};
#[cfg(feature = "reqwest")]
pub use reqwest_backend::ReqwestRequestBuilder as RequestBuilder;
#[cfg(feature = "reqwest")]
pub use reqwest_backend::{HttpClient, HttpClientBuilder, ReqwestRequestBuilder};

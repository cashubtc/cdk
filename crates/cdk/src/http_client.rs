//! Re-export HTTP client types from `cdk-http-client` via `cdk-common`.
//!
//! HTTP client abstraction for making HTTP requests.
//!
//! Backend selection is owned by `cdk-http-client`. Applications using
//! `cdk` defaults get the default `bitreq` backend. `cdk` no-default
//! `wallet` and `mint` builds also get `bitreq` through `cdk-common/http`.
//! To use `reqwest`, add a direct `cdk-http-client` dependency with the
//! `reqwest` feature; if both backends are enabled, `reqwest` takes
//! precedence:
//!
//! ```toml
//! [dependencies]
//! cdk = { version = "0.17.0", default-features = false, features = [
//!     "wallet",
//! ] }
//! cdk-http-client = { version = "0.17.0", default-features = false, features = [
//!     "reqwest",
//! ] }
//! ```
pub use cdk_common::{
    fetch, HttpClient, HttpClientBuilder, HttpError, RawResponse, RequestBuilder, Response,
};

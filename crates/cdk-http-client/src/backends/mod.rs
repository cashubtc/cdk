//! HTTP request builder backends
//!
//! Backend selection is additive: `reqwest` takes precedence when both the
//! `reqwest` and `bitreq` features are enabled. `reqwest` is a strict superset
//! (it adds SOCKS proxy and invalid-certificate support), so enabling it only
//! ever adds capability. `bitreq` is used only when `reqwest` is off. Keeping
//! the features additive means Cargo feature unification across a dependency
//! graph can never produce a build conflict.

const INVALID_URL_FOR_DEBUG: &str = "[INVALID URL]";

fn url_for_debug(value: &str) -> String {
    let Ok(mut url) = url::Url::parse(value) else {
        return INVALID_URL_FOR_DEBUG.to_owned();
    };

    if url.set_password(None).is_err() || url.set_username("").is_err() {
        return INVALID_URL_FOR_DEBUG.to_owned();
    }

    url.set_query(None);
    url.set_fragment(None);

    url.to_string()
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn install_rustls_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}

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

#[cfg(test)]
mod tests {
    use super::{url_for_debug, INVALID_URL_FOR_DEBUG};

    #[test]
    fn debug_url_redacts_credentials_query_and_fragment() {
        let secret = "url-secret";
        let url = format!("https://user:{secret}@example.com/api?token={secret}#{secret}");

        let debug_url = url_for_debug(&url);

        assert_eq!(debug_url, "https://example.com/api");
        assert!(!debug_url.contains(secret));
    }

    #[test]
    fn debug_url_redacts_malformed_input() {
        let secret = "malformed-url-secret";

        let debug_url = url_for_debug(&format!("not a URL containing {secret}"));

        assert_eq!(debug_url, INVALID_URL_FOR_DEBUG);
        assert!(!debug_url.contains(secret));
    }
}

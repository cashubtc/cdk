//! HTTP transport trait and implementations

use std::fmt::Debug;

use async_trait::async_trait;
use cashu::nuts::nut22::AuthToken;
use serde::Serialize;
use url::Url;

#[cfg(any(target_arch = "wasm32", feature = "bitreq", feature = "reqwest"))]
use crate::{HttpClient, HttpClientBuilder};
use crate::{HttpError, RawResponse};

/// Expected HTTP transport.
///
/// Callers that construct a transport implicitly may add a [`Default`] bound,
/// while configured transports can be supplied directly without implementing
/// a meaningless default configuration.
///
/// Implementations must not follow redirects. Most HTTP clients replay a
/// redirected POST as a bodyless GET, which would turn a receiver's 3xx into a
/// successful NUT-18 delivery that never carried the proofs. Return the 3xx
/// status, or [`HttpError::Redirect`] if the client refuses to hand it back.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait Transport: Send + Sync + Debug + Clone {
    /// Connect to a WebSocket endpoint using this transport.
    async fn ws_connect(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<(crate::ws::WsSender, crate::ws::WsReceiver), crate::ws::WsError> {
        crate::ws::connect(url, headers).await
    }

    /// Make the transport use a proxy.
    ///
    /// SOCKS proxy schemes such as `socks5h` are available only when this crate
    /// is built with the `reqwest` feature. The default `bitreq` backend accepts
    /// HTTP proxy URLs only.
    fn with_proxy(
        &mut self,
        proxy: Url,
        host_matcher: Option<&str>,
        accept_invalid_certs: bool,
    ) -> Result<(), HttpError>;

    /// Whether this transport accepts invalid TLS certificates.
    ///
    /// A transport that turns certificate verification off must report it here.
    /// The wallet refuses to hand NUT-18 proofs to an https receiver over an
    /// unverified connection, and the transport is the only thing that knows:
    /// a client wrapping a pre-configured transport cannot see how it was
    /// built.
    fn tls_verification_disabled(&self) -> bool {
        false
    }

    /// DNS resolver to get TXT records from a domain name.
    ///
    /// Transports that support DNS resolution should override this method. The
    /// default implementation keeps the trait API stable when the `bip353`
    /// feature is disabled.
    async fn resolve_dns_txt(&self, _domain: &str) -> Result<Vec<String>, HttpError> {
        Err(HttpError::Other(
            "DNS TXT resolution is not enabled for this transport".to_owned(),
        ))
    }

    /// HTTP GET request returning a raw response.
    async fn http_get(&self, url: Url, auth: Option<AuthToken>) -> Result<RawResponse, HttpError>;

    /// HTTP POST request with a JSON body returning a raw response.
    ///
    /// The caller decides what the status and body mean: mint calls decode with
    /// [`RawResponse::json_or_status_error`], while NUT-18 delivery only checks
    /// the status, since an empty or non-JSON success body is still a delivery.
    async fn http_post<P>(
        &self,
        url: Url,
        auth_token: Option<AuthToken>,
        payload: &P,
    ) -> Result<RawResponse, HttpError>
    where
        P: Serialize + Send + Sync;

    /// HTTP POST request with a form body returning a raw response.
    ///
    /// This cannot be expressed in terms of [`Transport::http_post`]: the body
    /// is form-encoded, and the endpoints that need it (OAuth token exchange)
    /// reject JSON. Transports that cannot send a form-encoded body keep the
    /// default and lose OIDC authentication only.
    async fn http_post_form<P>(
        &self,
        _url: Url,
        _auth_token: Option<AuthToken>,
        _payload: &P,
    ) -> Result<RawResponse, HttpError>
    where
        P: Serialize + Send + Sync,
    {
        Err(HttpError::Other(
            "form-encoded POST is not supported by this transport".to_owned(),
        ))
    }
}

/// Default async transport backed by the crate `HttpClient`.
#[cfg(any(target_arch = "wasm32", feature = "bitreq", feature = "reqwest"))]
#[derive(Debug, Clone)]
pub struct Async {
    inner: HttpClient,
    tls_verification_disabled: bool,
}

#[cfg(any(target_arch = "wasm32", feature = "bitreq", feature = "reqwest"))]
impl Default for Async {
    fn default() -> Self {
        Self {
            inner: HttpClient::builder()
                .no_redirects()
                .build()
                .expect("default no-redirect client"),
            tls_verification_disabled: false,
        }
    }
}

#[cfg(any(target_arch = "wasm32", feature = "bitreq", feature = "reqwest"))]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl Transport for Async {
    fn with_proxy(
        &mut self,
        proxy: Url,
        host_matcher: Option<&str>,
        accept_invalid_certs: bool,
    ) -> Result<(), HttpError> {
        let builder = HttpClientBuilder::default()
            .no_redirects()
            .danger_accept_invalid_certs(accept_invalid_certs);

        let builder = match host_matcher {
            Some(pattern) => builder.proxy_with_matcher(proxy, pattern)?,
            None => builder.proxy(proxy),
        };

        self.inner = builder.build()?;
        self.tls_verification_disabled = accept_invalid_certs;
        Ok(())
    }

    fn tls_verification_disabled(&self) -> bool {
        self.tls_verification_disabled
    }

    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    async fn resolve_dns_txt(&self, domain: &str) -> Result<Vec<String>, HttpError> {
        crate::dns::resolve_dns_txt(domain).await
    }

    async fn http_get(&self, url: Url, auth: Option<AuthToken>) -> Result<RawResponse, HttpError> {
        let url_str = url.to_string();
        let mut request = self.inner.get(&url_str);

        if let Some(auth) = auth {
            request = request.header(auth.header_key(), auth.to_string());
        }

        request.send().await
    }

    async fn http_post<P>(
        &self,
        url: Url,
        auth_token: Option<AuthToken>,
        payload: &P,
    ) -> Result<RawResponse, HttpError>
    where
        P: Serialize + Send + Sync,
    {
        let url_str = url.to_string();
        let mut request = self.inner.post(&url_str).json(payload);

        if let Some(auth) = auth_token {
            request = request.header(auth.header_key(), auth.to_string());
        }

        request.send().await
    }

    async fn http_post_form<P>(
        &self,
        url: Url,
        auth_token: Option<AuthToken>,
        payload: &P,
    ) -> Result<RawResponse, HttpError>
    where
        P: Serialize + Send + Sync,
    {
        let url_str = url.to_string();
        let mut request = self.inner.post(&url_str).form(payload);

        if let Some(auth) = auth_token {
            request = request.header(auth.header_key(), auth.to_string());
        }

        request.send().await
    }
}

#[cfg(all(
    feature = "bitreq",
    not(feature = "reqwest"),
    not(target_arch = "wasm32")
))]
/// Bitreq-backed transport implementation.
pub type BitreqTransport = Async;

#[cfg(all(feature = "reqwest", not(target_arch = "wasm32")))]
/// Reqwest-backed transport implementation.
pub type ReqwestTransport = Async;

#[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
mod tor_transport;

#[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
pub use self::tor_transport::TorAsync;

#[cfg(all(
    test,
    any(target_arch = "wasm32", feature = "bitreq", feature = "reqwest")
))]
mod tests {
    use super::*;

    #[test]
    fn default_transport_verifies_certificates() {
        assert!(!Async::default().tls_verification_disabled());
    }

    #[cfg(feature = "reqwest")]
    #[test]
    fn proxy_transport_reports_whether_verification_is_disabled() {
        let proxy = Url::parse("http://127.0.0.1:9050").expect("parse proxy url");

        let mut unverified = Async::default();
        unverified
            .with_proxy(proxy.clone(), None, true)
            .expect("configure proxy");
        assert!(unverified.tls_verification_disabled());

        let mut verified = Async::default();
        verified
            .with_proxy(proxy, None, false)
            .expect("configure proxy");
        assert!(!verified.tls_verification_disabled());
    }

    /// The flag is set only after the build succeeds, so a backend that
    /// refuses invalid certificates does not end up claiming it accepts them.
    #[cfg(all(feature = "bitreq", not(feature = "reqwest")))]
    #[test]
    fn bitreq_proxy_transport_refuses_disabled_verification() {
        let proxy = Url::parse("http://127.0.0.1:9050").expect("parse proxy url");

        let mut transport = Async::default();
        transport
            .with_proxy(proxy, None, true)
            .expect_err("bitreq cannot accept invalid certificates");

        assert!(!transport.tls_verification_disabled());
    }
}

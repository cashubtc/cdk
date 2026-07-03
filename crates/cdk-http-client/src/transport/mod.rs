//! HTTP transport trait and implementations

use std::fmt::Debug;

use async_trait::async_trait;
use cashu::nuts::nut22::AuthToken;
use serde::de::DeserializeOwned;
use serde::Serialize;
use url::Url;

use crate::HttpError;
#[cfg(any(target_arch = "wasm32", feature = "bitreq", feature = "reqwest"))]
use crate::{HttpClient, HttpClientBuilder};

/// Expected HTTP transport
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait Transport: Default + Send + Sync + Debug + Clone {
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

    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    /// DNS resolver to get TXT records from a domain name.
    async fn resolve_dns_txt(&self, domain: &str) -> Result<Vec<String>, HttpError>;

    /// HTTP GET request.
    async fn http_get<R>(&self, url: Url, auth: Option<AuthToken>) -> Result<R, HttpError>
    where
        R: DeserializeOwned;

    /// HTTP POST request.
    async fn http_post<P, R>(
        &self,
        url: Url,
        auth_token: Option<AuthToken>,
        payload: &P,
    ) -> Result<R, HttpError>
    where
        P: Serialize + Send + Sync,
        R: DeserializeOwned;
}

/// Default async transport backed by the crate `HttpClient`.
#[cfg(any(target_arch = "wasm32", feature = "bitreq", feature = "reqwest"))]
#[derive(Debug, Clone)]
pub struct Async {
    inner: HttpClient,
    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    resolver: std::sync::Arc<
        hickory_resolver::Resolver<hickory_resolver::name_server::TokioConnectionProvider>,
    >,
}

#[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
fn default_resolver(
) -> hickory_resolver::Resolver<hickory_resolver::name_server::TokioConnectionProvider> {
    use hickory_resolver::config::{ResolverConfig, ResolverOpts};
    use hickory_resolver::name_server::TokioConnectionProvider;
    use hickory_resolver::Resolver;

    let mut resolver_opts = ResolverOpts::default();
    resolver_opts.validate = true;

    Resolver::builder_with_config(
        ResolverConfig::default(),
        TokioConnectionProvider::default(),
    )
    .with_options(resolver_opts)
    .build()
}

#[cfg(any(target_arch = "wasm32", feature = "bitreq", feature = "reqwest"))]
impl Default for Async {
    fn default() -> Self {
        #[cfg(all(not(target_arch = "wasm32"), feature = "bip353"))]
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }

        Self {
            inner: HttpClient::builder()
                .no_redirects()
                .build()
                .expect("default no-redirect client"),
            #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
            resolver: std::sync::Arc::new(default_resolver()),
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
        Ok(())
    }

    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    async fn resolve_dns_txt(&self, domain: &str) -> Result<Vec<String>, HttpError> {
        use std::str::FromStr;

        let name = hickory_resolver::Name::from_str(domain)
            .map_err(|e| HttpError::Other(format!("Invalid domain name: {}", e)))?;

        Ok(self
            .resolver
            .txt_lookup(name)
            .await
            .map_err(|e| HttpError::Other(e.to_string()))?
            .into_iter()
            .map(|txt| {
                txt.txt_data()
                    .iter()
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                    .collect::<Vec<_>>()
                    .join("")
            })
            .collect::<Vec<_>>())
    }

    async fn http_get<R>(&self, url: Url, auth: Option<AuthToken>) -> Result<R, HttpError>
    where
        R: DeserializeOwned,
    {
        let url_str = url.to_string();
        let mut request = self.inner.get(&url_str);

        if let Some(auth) = auth {
            request = request.header(auth.header_key(), auth.to_string());
        }

        request.send_json::<R>().await
    }

    async fn http_post<P, R>(
        &self,
        url: Url,
        auth_token: Option<AuthToken>,
        payload: &P,
    ) -> Result<R, HttpError>
    where
        P: Serialize + Send + Sync,
        R: DeserializeOwned,
    {
        let url_str = url.to_string();
        let mut request = self.inner.post(&url_str).json(payload);

        if let Some(auth) = auth_token {
            request = request.header(auth.header_key(), auth.to_string());
        }

        request.send_json::<R>().await
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

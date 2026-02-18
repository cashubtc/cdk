//! HTTP Transport trait with a default implementation
use std::fmt::Debug;

use cdk_common::{AuthToken, HttpClient, HttpClientBuilder};
use cdk_http_client::RequestBuilderExt;
#[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
use hickory_resolver::config::ResolverConfig;
#[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
use hickory_resolver::name_server::TokioConnectionProvider;
#[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
use hickory_resolver::Resolver;
use serde::de::DeserializeOwned;
use serde::Serialize;
use url::Url;

use super::Error;
use crate::error::ErrorResponse;

/// Expected HTTP Transport
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait Transport: Default + Send + Sync + Debug + Clone {
    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    /// DNS resolver to get a TXT record from a domain name
    async fn resolve_dns_txt(&self, _domain: &str) -> Result<Vec<String>, Error>;

    /// Make the transport to use a given proxy
    fn with_proxy(
        &mut self,
        proxy: url::Url,
        host_matcher: Option<&str>,
        accept_invalid_certs: bool,
    ) -> Result<(), super::Error>;

    /// HTTP Get request
    async fn http_get<R>(
        &self,
        url: url::Url,
        auth: Option<cdk_common::AuthToken>,
    ) -> Result<R, super::Error>
    where
        R: serde::de::DeserializeOwned;

    /// HTTP Post request
    async fn http_post<P, R>(
        &self,
        url: url::Url,
        auth_token: Option<cdk_common::AuthToken>,
        payload: &P,
    ) -> Result<R, super::Error>
    where
        P: serde::Serialize + ?Sized + Send + Sync,
        R: serde::de::DeserializeOwned;
}

/// Async transport for Http
#[derive(Debug, Clone)]
pub struct Async {
    inner: HttpClient,
}

impl Default for Async {
    fn default() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }

        Self {
            inner: HttpClient::new(),
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl Transport for Async {
    #[cfg(target_arch = "wasm32")]
    fn with_proxy(
        &mut self,
        _proxy: Url,
        _host_matcher: Option<&str>,
        _accept_invalid_certs: bool,
    ) -> Result<(), Error> {
        panic!("Not supported in wasm");
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn with_proxy(
        &mut self,
        proxy: Url,
        host_matcher: Option<&str>,
        accept_invalid_certs: bool,
    ) -> Result<(), Error> {
        let builder =
            HttpClientBuilder::default().danger_accept_invalid_certs(accept_invalid_certs);

        let builder = match host_matcher {
            Some(pattern) => {
                // When a matcher is provided, only apply the proxy to matched hosts
                builder
                    .proxy_with_matcher(proxy, pattern)
                    .map_err(|e| Error::Custom(e.to_string()))?
            }
            // Apply proxy to all requests when no matcher is provided
            None => builder.proxy(proxy),
        };

        self.inner = builder
            .build()
            .map_err(|e| Error::HttpError(None, e.to_string()))?;
        Ok(())
    }

    /// DNS resolver to get a TXT record from a domain name
    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    async fn resolve_dns_txt(&self, domain: &str) -> Result<Vec<String>, Error> {
        let resolver = Resolver::builder_with_config(
            ResolverConfig::default(),
            TokioConnectionProvider::default(),
        )
        .build();

        Ok(resolver
            .txt_lookup(domain)
            .await
            .map_err(|e| Error::Custom(e.to_string()))?
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

    async fn http_get<R>(&self, url: Url, auth: Option<AuthToken>) -> Result<R, Error>
    where
        R: DeserializeOwned,
    {
        let url_str = url.to_string();
        let mut request = self.inner.get(&url_str);

        if let Some(auth) = auth {
            request = request.header(auth.header_key(), auth.to_string());
        }

        let response = request
            .send()
            .await
            .map_err(|e| Error::HttpError(None, e.to_string()))?
            .text()
            .await
            .map_err(|e| Error::HttpError(None, e.to_string()))?;

        serde_json::from_str::<R>(&response).map_err(|err| {
            tracing::warn!("Http Response error: {}", err);
            match ErrorResponse::from_json(&response) {
                Ok(ok) => <ErrorResponse as Into<Error>>::into(ok),
                Err(err) => err.into(),
            }
        })
    }

    async fn http_post<P, R>(
        &self,
        url: Url,
        auth_token: Option<AuthToken>,
        payload: &P,
    ) -> Result<R, Error>
    where
        P: Serialize + ?Sized + Send + Sync,
        R: DeserializeOwned,
    {
        let url_str = url.to_string();
        let mut request = self.inner.post(&url_str).json(&payload);

        if let Some(auth) = auth_token {
            request = request.header(auth.header_key(), auth.to_string());
        }

        let response = request
            .send()
            .await
            .map_err(|e| Error::HttpError(None, e.to_string()))?;

        let response = response
            .text()
            .await
            .map_err(|e| Error::HttpError(None, e.to_string()))?;

        serde_json::from_str::<R>(&response).map_err(|err| {
            tracing::warn!("Http Response error: {}", err);
            tracing::debug!("{:?}", response);
            match ErrorResponse::from_json(&response) {
                Ok(ok) => <ErrorResponse as Into<Error>>::into(ok),
                Err(err) => err.into(),
            }
        })
    }
}

#[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
pub mod tor_transport;

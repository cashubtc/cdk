//! HTTP Transport trait with a default implementation
use std::collections::HashMap;
use std::fmt::Debug;

use bitreq::{Client, Proxy, Request, RequestExt};
use cdk_common::AuthToken;
#[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
use hickory_resolver::config::ResolverConfig;
#[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
use hickory_resolver::name_server::TokioConnectionProvider;
#[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
use hickory_resolver::Resolver;
use regex::Regex;
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

#[derive(Debug, Clone)]
struct ProxyWrapper {
    proxy: Proxy,
    _accept_invalid_certs: bool,
}

/// Async transport for Http
#[derive(Clone)]
pub struct Async {
    client: Client,
    proxy_per_url: HashMap<String, (Regex, ProxyWrapper)>,
    all_proxy: Option<ProxyWrapper>,
}

impl Async {
    fn prepare_request(&self, req: Request, url: Url, auth: Option<AuthToken>) -> Request {
        let proxy = {
            let url = url.to_string();
            let mut proxy = None;
            for (pattern, proxy_wrapper) in self.proxy_per_url.values() {
                if pattern.is_match(&url) {
                    proxy = Some(proxy_wrapper.proxy.clone());
                }
            }

            if proxy.is_some() {
                proxy
            } else {
                self.all_proxy.as_ref().map(|x| x.proxy.clone())
            }
        };

        let request = if let Some(proxy) = proxy {
            req.with_proxy(proxy)
        } else {
            req
        };

        if let Some(auth) = auth {
            request.with_header(auth.header_key(), auth.to_string())
        } else {
            request
        }
    }
}

impl Debug for Async {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP Async client")
    }
}

impl Default for Async {
    fn default() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }

        Self {
            client: Client::new(10),
            proxy_per_url: HashMap::new(),
            all_proxy: None,
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
        let proxy = ProxyWrapper {
            proxy: bitreq::Proxy::new_http(proxy).map_err(|_| Error::Internal)?,
            _accept_invalid_certs: accept_invalid_certs,
        };
        if let Some((key, pattern)) = host_matcher
            .map(|pattern| {
                regex::Regex::new(pattern)
                    .map(|regex| (pattern.to_owned(), regex))
                    .map_err(|e| Error::Custom(e.to_string()))
            })
            .transpose()?
        {
            self.proxy_per_url.insert(key, (pattern, proxy));
        } else {
            self.all_proxy = Some(proxy);
        }

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
        let response = self
            .prepare_request(bitreq::get(url.clone()), url, auth)
            .send_async_with_client(&self.client)
            .await
            .map_err(|e| Error::HttpError(None, e.to_string()))?;

        if response.status_code != 200 {
            return Err(Error::HttpError(
                Some(response.status_code as u16),
                "".to_string(),
            ));
        }

        serde_json::from_slice::<R>(response.as_bytes()).map_err(|err| {
            tracing::warn!("Http Response error: {}", err);
            match ErrorResponse::from_slice(response.as_bytes()) {
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
        let response = self
            .prepare_request(bitreq::post(url.clone()), url, auth_token)
            .with_body(serde_json::to_string(payload).map_err(Error::SerdeJsonError)?)
            .with_header(
                "Content-Type".to_string(),
                "application/json; charset=UTF-8".to_string(),
            )
            .send_async_with_client(&self.client)
            .await
            .map_err(|e| Error::HttpError(None, e.to_string()))?;

        if response.status_code != 200 {
            return Err(Error::HttpError(
                Some(response.status_code as u16),
                "".to_string(),
            ));
        }

        serde_json::from_slice::<R>(response.as_bytes()).map_err(|err| {
            tracing::warn!("Http Response error: {}", err);
            match ErrorResponse::from_slice(response.as_bytes()) {
                Ok(ok) => <ErrorResponse as Into<Error>>::into(ok),
                Err(err) => err.into(),
            }
        })
    }
}

#[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
pub mod tor_transport;

//! HTTP Transport trait with a default implementation
use std::fmt::Debug;

use cdk_common::AuthToken;
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::Serialize;
use url::Url;

use super::Error;
use crate::error::ErrorResponse;

/// Expected HTTP Transport
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait Transport: Default + Send + Sync + Debug + Clone {
    /// Make the transport to use a given proxy
    fn with_proxy(
        &mut self,
        proxy: Url,
        host_matcher: Option<&str>,
        accept_invalid_certs: bool,
    ) -> Result<(), Error>;

    /// HTTP Get request
    async fn http_get<R>(&self, url: Url, auth: Option<AuthToken>) -> Result<R, Error>
    where
        R: DeserializeOwned;

    /// HTTP Post request
    async fn http_post<P, R>(
        &self,
        url: Url,
        auth_token: Option<AuthToken>,
        payload: &P,
    ) -> Result<R, Error>
    where
        P: Serialize + ?Sized + Send + Sync,
        R: DeserializeOwned;
}

/// Async transport for Http
#[derive(Debug, Clone)]
pub struct Async {
    inner: Client,
}

impl Default for Async {
    fn default() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }

        Self {
            inner: Client::new(),
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
        let regex = host_matcher
            .map(regex::Regex::new)
            .transpose()
            .map_err(|e| Error::Custom(e.to_string()))?;
        self.inner = reqwest::Client::builder()
            .proxy(reqwest::Proxy::custom(move |url| {
                if let Some(matcher) = regex.as_ref() {
                    if let Some(host) = url.host_str() {
                        if matcher.is_match(host) {
                            return Some(proxy.clone());
                        }
                    }
                }
                None
            }))
            .danger_accept_invalid_certs(accept_invalid_certs) // Allow self-signed certs
            .build()
            .map_err(|e| {
                Error::HttpError(
                    e.status().map(|status_code| status_code.as_u16()),
                    e.to_string(),
                )
            })?;
        Ok(())
    }

    async fn http_get<R>(&self, url: Url, auth: Option<AuthToken>) -> Result<R, Error>
    where
        R: DeserializeOwned,
    {
        let mut request = self.inner.get(url);

        if let Some(auth) = auth {
            request = request.header(auth.header_key(), auth.to_string());
        }

        let response = request
            .send()
            .await
            .map_err(|e| {
                Error::HttpError(
                    e.status().map(|status_code| status_code.as_u16()),
                    e.to_string(),
                )
            })?
            .text()
            .await
            .map_err(|e| {
                Error::HttpError(
                    e.status().map(|status_code| status_code.as_u16()),
                    e.to_string(),
                )
            })?;

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
        let mut request = self.inner.post(url).json(&payload);

        if let Some(auth) = auth_token {
            request = request.header(auth.header_key(), auth.to_string());
        }

        let response = request.send().await.map_err(|e| {
            Error::HttpError(
                e.status().map(|status_code| status_code.as_u16()),
                e.to_string(),
            )
        })?;

        let response = response.text().await.map_err(|e| {
            Error::HttpError(
                e.status().map(|status_code| status_code.as_u16()),
                e.to_string(),
            )
        })?;

        serde_json::from_str::<R>(&response).map_err(|err| {
            tracing::warn!("Http Response error: {}", err);
            match ErrorResponse::from_json(&response) {
                Ok(ok) => <ErrorResponse as Into<Error>>::into(ok),
                Err(err) => err.into(),
            }
        })
    }
}

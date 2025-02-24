//! Wallet client

use std::fmt::Debug;

use async_trait::async_trait;
use cdk_common::kvac::{
    BootstrapRequest, BootstrapResponse, KvacKeySet, KvacKeysResponse, KvacKeysetResponse,
    KvacMeltBolt11Request, KvacMeltBolt11Response, KvacMintBolt11Request, KvacMintBolt11Response,
    KvacRestoreRequest, KvacRestoreResponse, KvacSwapRequest, KvacSwapResponse,
};
use reqwest::{Client, IntoUrl};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tracing::instrument;
#[cfg(not(target_arch = "wasm32"))]
use url::Url;

use super::Error;
use crate::error::ErrorResponse;
use crate::mint_url::MintUrl;
use crate::nuts::{
    CheckStateRequest, CheckStateResponse, Id, KeySet, KeysResponse, KeysetResponse,
    MeltBolt11Request, MeltQuoteBolt11Request, MeltQuoteBolt11Response, MintBolt11Request,
    MintBolt11Response, MintInfo, MintQuoteBolt11Request, MintQuoteBolt11Response, RestoreRequest,
    RestoreResponse, SwapRequest, SwapResponse,
};

/// Http Client
#[derive(Debug, Clone)]
pub struct HttpClient {
    inner: Client,
    mint_url: MintUrl,
}

impl HttpClient {
    /// Create new [`HttpClient`]
    pub fn new(mint_url: MintUrl) -> Self {
        Self {
            inner: Client::new(),
            mint_url,
        }
    }

    #[inline]
    async fn http_get<U: IntoUrl, R: DeserializeOwned>(&self, url: U) -> Result<R, Error> {
        let response = self
            .inner
            .get(url)
            .send()
            .await
            .map_err(|e| Error::HttpError(e.to_string()))?
            .text()
            .await
            .map_err(|e| Error::HttpError(e.to_string()))?;

        serde_json::from_str::<R>(&response).map_err(|err| {
            tracing::warn!("Http Response error: {}", err);
            match ErrorResponse::from_json(&response) {
                Ok(ok) => <ErrorResponse as Into<Error>>::into(ok),
                Err(err) => err.into(),
            }
        })
    }

    #[inline]
    async fn http_post<U: IntoUrl, P: Serialize + ?Sized, R: DeserializeOwned>(
        &self,
        url: U,
        payload: &P,
    ) -> Result<R, Error> {
        let response = self
            .inner
            .post(url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| Error::HttpError(e.to_string()))?
            .text()
            .await
            .map_err(|e| Error::HttpError(e.to_string()))?;

        serde_json::from_str::<R>(&response).map_err(|err| {
            tracing::warn!("Http Response error: {}", err);
            match ErrorResponse::from_json(&response) {
                Ok(ok) => <ErrorResponse as Into<Error>>::into(ok),
                Err(err) => err.into(),
            }
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    /// Create new [`HttpClient`] with a proxy for specific TLDs.
    /// Specifying `None` for `host_matcher` will use the proxy for all
    /// requests.
    pub fn with_proxy(
        mint_url: MintUrl,
        proxy: Url,
        host_matcher: Option<&str>,
        accept_invalid_certs: bool,
    ) -> Result<Self, Error> {
        let regex = host_matcher
            .map(regex::Regex::new)
            .transpose()
            .map_err(|e| Error::Custom(e.to_string()))?;
        let client = reqwest::Client::builder()
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
            .map_err(|e| Error::HttpError(e.to_string()))?;

        Ok(Self {
            inner: client,
            mint_url,
        })
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl MintConnector for HttpClient {
    /// Get Active Mint Keys [NUT-01]
    #[instrument(skip(self), fields(mint_url = %self.mint_url))]
    async fn get_mint_keys(&self) -> Result<Vec<KeySet>, Error> {
        let url = self.mint_url.join_paths(&["v1", "keys"])?;
        Ok(self.http_get::<_, KeysResponse>(url).await?.keysets)
    }

    /// Get Active Mint Kvac Keys
    #[instrument(skip(self), fields(mint_url = %self.mint_url))]
    async fn get_mint_kvac_keys(&self) -> Result<Vec<KvacKeySet>, Error> {
        let url = self.mint_url.join_paths(&["v2", "kvac", "keys"])?;
        Ok(self
            .http_get::<_, KvacKeysResponse>(url)
            .await?
            .kvac_keysets)
    }

    /// Get Keyset Keys [NUT-01]
    #[instrument(skip(self), fields(mint_url = %self.mint_url))]
    async fn get_mint_keyset(&self, keyset_id: Id) -> Result<KeySet, Error> {
        let url = self
            .mint_url
            .join_paths(&["v1", "keys", &keyset_id.to_string()])?;
        self.http_get::<_, KeysResponse>(url)
            .await?
            .keysets
            .drain(0..1)
            .next()
            .ok_or_else(|| Error::UnknownKeySet)
    }

    /// Get Keyset Keys [NUT-01]
    #[instrument(skip(self), fields(mint_url = %self.mint_url))]
    async fn get_mint_kvac_keyset(&self, keyset_id: Id) -> Result<KvacKeySet, Error> {
        let url = self
            .mint_url
            .join_paths(&["v2", "kvac", "keys", &keyset_id.to_string()])?;
        self.http_get::<_, KvacKeysResponse>(url)
            .await?
            .kvac_keysets
            .drain(0..1)
            .next()
            .ok_or_else(|| Error::UnknownKeySet)
    }

    /// Get Keysets [NUT-02]
    #[instrument(skip(self), fields(mint_url = %self.mint_url))]
    async fn get_mint_keysets(&self) -> Result<KeysetResponse, Error> {
        let url = self.mint_url.join_paths(&["v1", "keysets"])?;
        self.http_get(url).await
    }

    /// Get Kvac Keysets
    #[instrument(skip(self), fields(mint_url = %self.mint_url))]
    async fn get_mint_kvac_keysets(&self) -> Result<KvacKeysetResponse, Error> {
        let url = self.mint_url.join_paths(&["v2", "kvac", "keysets"])?;
        self.http_get(url).await
    }

    /// Mint Quote [NUT-04]
    #[instrument(skip(self), fields(mint_url = %self.mint_url))]
    async fn post_mint_quote(
        &self,
        request: MintQuoteBolt11Request,
    ) -> Result<MintQuoteBolt11Response<String>, Error> {
        let url = self
            .mint_url
            .join_paths(&["v1", "mint", "quote", "bolt11"])?;
        self.http_post(url, &request).await
    }

    /// Mint Quote status
    #[instrument(skip(self), fields(mint_url = %self.mint_url))]
    async fn get_mint_quote_status(
        &self,
        quote_id: &str,
    ) -> Result<MintQuoteBolt11Response<String>, Error> {
        let url = self
            .mint_url
            .join_paths(&["v1", "mint", "quote", "bolt11", quote_id])?;

        self.http_get(url).await
    }

    /// Mint Tokens [NUT-04]
    #[instrument(skip(self, request), fields(mint_url = %self.mint_url))]
    async fn post_mint(
        &self,
        request: MintBolt11Request<String>,
    ) -> Result<MintBolt11Response, Error> {
        let url = self.mint_url.join_paths(&["v1", "mint", "bolt11"])?;
        self.http_post(url, &request).await
    }

    /// Mint Kvac Coins
    async fn post_kvac_mint(
        &self,
        request: KvacMintBolt11Request<String>,
    ) -> Result<KvacMintBolt11Response, Error> {
        let url = self
            .mint_url
            .join_paths(&["v2", "kvac", "mint", "bolt11"])?;
        self.http_post(url, &request).await
    }

    /// Melt Quote [NUT-05]
    #[instrument(skip(self, request), fields(mint_url = %self.mint_url))]
    async fn post_melt_quote(
        &self,
        request: MeltQuoteBolt11Request,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        let url = self
            .mint_url
            .join_paths(&["v1", "melt", "quote", "bolt11"])?;
        self.http_post(url, &request).await
    }

    /// Melt Quote Status
    #[instrument(skip(self), fields(mint_url = %self.mint_url))]
    async fn get_melt_quote_status(
        &self,
        quote_id: &str,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        let url = self
            .mint_url
            .join_paths(&["v1", "melt", "quote", "bolt11", quote_id])?;

        self.http_get(url).await
    }

    /// Melt [NUT-05]
    /// [Nut-08] Lightning fee return if outputs defined
    #[instrument(skip(self, request), fields(mint_url = %self.mint_url))]
    async fn post_melt(
        &self,
        request: MeltBolt11Request<String>,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        let url = self.mint_url.join_paths(&["v1", "melt", "bolt11"])?;
        self.http_post(url, &request).await
    }

    /// Melt KVAC coins
    #[instrument(skip(self, request), fields(mint_url = %self.mint_url))]
    async fn post_kvac_melt(
        &self,
        request: KvacMeltBolt11Request<String>,
    ) -> Result<KvacMeltBolt11Response, Error> {
        let url = self
            .mint_url
            .join_paths(&["v2", "kvac", "melt", "bolt11"])?;
        self.http_post(url, &request).await
    }

    /// Swap Token [NUT-03]
    #[instrument(skip(self, swap_request), fields(mint_url = %self.mint_url))]
    async fn post_swap(&self, swap_request: SwapRequest) -> Result<SwapResponse, Error> {
        let url = self.mint_url.join_paths(&["v1", "swap"])?;
        self.http_post(url, &swap_request).await
    }

    /// Swap Token [NUT-03]
    #[instrument(skip(self, swap_request), fields(mint_url = %self.mint_url))]
    async fn post_kvac_swap(
        &self,
        swap_request: KvacSwapRequest,
    ) -> Result<KvacSwapResponse, Error> {
        let url = self.mint_url.join_paths(&["v2", "kvac", "swap"])?;
        self.http_post(url, &swap_request).await
    }

    /// Get Mint Info [NUT-06]
    #[instrument(skip(self), fields(mint_url = %self.mint_url))]
    async fn get_mint_info(&self) -> Result<MintInfo, Error> {
        let url = self.mint_url.join_paths(&["v1", "info"])?;
        self.http_get(url).await
    }

    /// Spendable check [NUT-07]
    #[instrument(skip(self, request), fields(mint_url = %self.mint_url))]
    async fn post_check_state(
        &self,
        request: CheckStateRequest,
    ) -> Result<CheckStateResponse, Error> {
        let url = self.mint_url.join_paths(&["v1", "checkstate"])?;
        self.http_post(url, &request).await
    }

    /// Restore request [NUT-13]
    #[instrument(skip(self, request), fields(mint_url = %self.mint_url))]
    async fn post_restore(&self, request: RestoreRequest) -> Result<RestoreResponse, Error> {
        let url = self.mint_url.join_paths(&["v1", "restore"])?;
        self.http_post(url, &request).await
    }

    /// KVAC Restore Request
    #[instrument(skip(self, request), fields(mint_url = %self.mint_url))]
    async fn post_kvac_restore(
        &self,
        request: KvacRestoreRequest,
    ) -> Result<KvacRestoreResponse, Error> {
        let url = self.mint_url.join_paths(&["v2", "kvac", "restore"])?;
        self.http_post(url, &request).await
    }

    /// KVAC Bootstrap
    async fn post_bootstrap(&self, request: BootstrapRequest) -> Result<BootstrapResponse, Error> {
        let url = self.mint_url.join_paths(&["v2", "kvac", "bootstrap"])?;
        self.http_post(url, &request).await
    }
}

/// Interface that connects a wallet to a mint. Typically represents an [HttpClient].
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait MintConnector: Debug {
    /// Get Active Mint Keys [NUT-01]
    async fn get_mint_keys(&self) -> Result<Vec<KeySet>, Error>;
    /// Get Active Mint Kvac Keys
    async fn get_mint_kvac_keys(&self) -> Result<Vec<KvacKeySet>, Error> {
        Err(Error::NotImplemented)
    }
    /// Get Keyset Keys [NUT-01]
    async fn get_mint_keyset(&self, keyset_id: Id) -> Result<KeySet, Error>;
    /// Get Keyset Kvac Keys
    async fn get_mint_kvac_keyset(&self, _keyset_id: Id) -> Result<KvacKeySet, Error> {
        Err(Error::NotImplemented)
    }
    /// Get Keysets [NUT-02]
    async fn get_mint_keysets(&self) -> Result<KeysetResponse, Error>;
    /// Get Kvac Keysets
    async fn get_mint_kvac_keysets(&self) -> Result<KvacKeysetResponse, Error> {
        Err(Error::NotImplemented)
    }
    /// Mint Quote [NUT-04]
    async fn post_mint_quote(
        &self,
        request: MintQuoteBolt11Request,
    ) -> Result<MintQuoteBolt11Response<String>, Error>;
    /// Mint Quote status
    async fn get_mint_quote_status(
        &self,
        quote_id: &str,
    ) -> Result<MintQuoteBolt11Response<String>, Error>;
    /// Mint Tokens [NUT-04]
    async fn post_mint(
        &self,
        request: MintBolt11Request<String>,
    ) -> Result<MintBolt11Response, Error>;
    /// Mint Kvac Coins
    async fn post_kvac_mint(
        &self,
        _request: KvacMintBolt11Request<String>,
    ) -> Result<KvacMintBolt11Response, Error> {
        Err(Error::NotImplemented)
    }
    /// Melt Quote [NUT-05]
    async fn post_melt_quote(
        &self,
        request: MeltQuoteBolt11Request,
    ) -> Result<MeltQuoteBolt11Response<String>, Error>;
    /// Melt Quote Status
    async fn get_melt_quote_status(
        &self,
        quote_id: &str,
    ) -> Result<MeltQuoteBolt11Response<String>, Error>;
    /// Melt [NUT-05]
    /// [Nut-08] Lightning fee return if outputs defined
    async fn post_melt(
        &self,
        request: MeltBolt11Request<String>,
    ) -> Result<MeltQuoteBolt11Response<String>, Error>;
    /// Melt KVAC coins
    async fn post_kvac_melt(
        &self,
        _request: KvacMeltBolt11Request<String>,
    ) -> Result<KvacMeltBolt11Response, Error> {
        Err(Error::NotImplemented)
    }
    /// Split Token [NUT-06]
    async fn post_swap(&self, request: SwapRequest) -> Result<SwapResponse, Error>;
    /// Swap KVAC Coins
    async fn post_kvac_swap(&self, _request: KvacSwapRequest) -> Result<KvacSwapResponse, Error> {
        Err(Error::NotImplemented)
    }
    /// Get Mint Info [NUT-06]
    async fn get_mint_info(&self) -> Result<MintInfo, Error>;
    /// Spendable check [NUT-07]
    async fn post_check_state(
        &self,
        request: CheckStateRequest,
    ) -> Result<CheckStateResponse, Error>;
    /// Restore request [NUT-13]
    async fn post_restore(&self, request: RestoreRequest) -> Result<RestoreResponse, Error>;
    /// Restore KVAC
    async fn post_kvac_restore(
        &self,
        _request: KvacRestoreRequest,
    ) -> Result<KvacRestoreResponse, Error> {
        Err(Error::NotImplemented)
    }
    /// Kvac Bootstrap
    async fn post_bootstrap(&self, _request: BootstrapRequest) -> Result<BootstrapResponse, Error> {
        Err(Error::NotImplemented)
    }
}

//! Wallet client

use std::fmt::Debug;

use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;
use tracing::instrument;
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
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpClient {
    /// Create new [`HttpClient`]
    pub fn new() -> Self {
        Self {
            inner: Client::new(),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    /// Create new [`HttpClient`] with a proxy for specific TLDs.
    /// Specifying `None` for `host_matcher` will use the proxy for all
    /// requests.
    pub fn with_proxy(
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
            .build()?;

        Ok(Self { inner: client })
    }
}

#[async_trait]
impl HttpClientMethods for HttpClient {
    /// Get Active Mint Keys [NUT-01]
    #[instrument(skip(self), fields(mint_url = %mint_url))]
    async fn get_mint_keys(&self, mint_url: MintUrl) -> Result<Vec<KeySet>, Error> {
        let url = mint_url.join_paths(&["v1", "keys"])?;
        let keys = self.inner.get(url).send().await?.json::<Value>().await?;

        match serde_json::from_value::<KeysResponse>(keys.clone()) {
            Ok(keys_response) => Ok(keys_response.keysets),
            Err(_) => Err(ErrorResponse::from_value(keys)?.into()),
        }
    }

    /// Get Keyset Keys [NUT-01]
    #[instrument(skip(self), fields(mint_url = %mint_url))]
    async fn get_mint_keyset(&self, mint_url: MintUrl, keyset_id: Id) -> Result<KeySet, Error> {
        let url = mint_url.join_paths(&["v1", "keys", &keyset_id.to_string()])?;
        let keys = self.inner.get(url).send().await?.json::<Value>().await?;

        match serde_json::from_value::<KeysResponse>(keys.clone()) {
            Ok(keys_response) => Ok(keys_response.keysets[0].clone()),
            Err(_) => Err(ErrorResponse::from_value(keys)?.into()),
        }
    }

    /// Get Keysets [NUT-02]
    #[instrument(skip(self), fields(mint_url = %mint_url))]
    async fn get_mint_keysets(&self, mint_url: MintUrl) -> Result<KeysetResponse, Error> {
        let url = mint_url.join_paths(&["v1", "keysets"])?;
        let res = self.inner.get(url).send().await?.json::<Value>().await?;

        match serde_json::from_value::<KeysetResponse>(res.clone()) {
            Ok(keyset_response) => Ok(keyset_response),
            Err(_) => Err(ErrorResponse::from_value(res)?.into()),
        }
    }

    /// Mint Quote [NUT-04]
    #[instrument(skip(self), fields(mint_url = %mint_url))]
    async fn post_mint_quote(
        &self,
        mint_url: MintUrl,
        request: MintQuoteBolt11Request,
    ) -> Result<MintQuoteBolt11Response, Error> {
        let url = mint_url.join_paths(&["v1", "mint", "quote", "bolt11"])?;

        let res = self
            .inner
            .post(url)
            .json(&request)
            .send()
            .await?
            .json::<Value>()
            .await?;

        match serde_json::from_value::<MintQuoteBolt11Response>(res.clone()) {
            Ok(mint_quote_response) => Ok(mint_quote_response),
            Err(err) => {
                tracing::warn!("{}", err);
                Err(ErrorResponse::from_value(res)?.into())
            }
        }
    }

    /// Mint Quote status
    #[instrument(skip(self), fields(mint_url = %mint_url))]
    async fn get_mint_quote_status(
        &self,
        mint_url: MintUrl,
        quote_id: &str,
    ) -> Result<MintQuoteBolt11Response, Error> {
        let url = mint_url.join_paths(&["v1", "mint", "quote", "bolt11", quote_id])?;

        let res = self.inner.get(url).send().await?.json::<Value>().await?;

        match serde_json::from_value::<MintQuoteBolt11Response>(res.clone()) {
            Ok(mint_quote_response) => Ok(mint_quote_response),
            Err(err) => {
                tracing::warn!("{}", err);
                Err(ErrorResponse::from_value(res)?.into())
            }
        }
    }

    /// Mint Tokens [NUT-04]
    #[instrument(skip(self, request), fields(mint_url = %mint_url))]
    async fn post_mint(
        &self,
        mint_url: MintUrl,
        request: MintBolt11Request,
    ) -> Result<MintBolt11Response, Error> {
        let url = mint_url.join_paths(&["v1", "mint", "bolt11"])?;

        let res = self
            .inner
            .post(url)
            .json(&request)
            .send()
            .await?
            .json::<Value>()
            .await?;

        match serde_json::from_value::<MintBolt11Response>(res.clone()) {
            Ok(mint_quote_response) => Ok(mint_quote_response),
            Err(_) => Err(ErrorResponse::from_value(res)?.into()),
        }
    }

    /// Melt Quote [NUT-05]
    #[instrument(skip(self, request), fields(mint_url = %mint_url))]
    async fn post_melt_quote(
        &self,
        mint_url: MintUrl,
        request: MeltQuoteBolt11Request,
    ) -> Result<MeltQuoteBolt11Response, Error> {
        let url = mint_url.join_paths(&["v1", "melt", "quote", "bolt11"])?;

        let res = self
            .inner
            .post(url)
            .json(&request)
            .send()
            .await?
            .json::<Value>()
            .await?;

        match serde_json::from_value::<MeltQuoteBolt11Response>(res.clone()) {
            Ok(melt_quote_response) => Ok(melt_quote_response),
            Err(_) => Err(ErrorResponse::from_value(res)?.into()),
        }
    }

    /// Melt Quote Status
    #[instrument(skip(self), fields(mint_url = %mint_url))]
    async fn get_melt_quote_status(
        &self,
        mint_url: MintUrl,
        quote_id: &str,
    ) -> Result<MeltQuoteBolt11Response, Error> {
        let url = mint_url.join_paths(&["v1", "melt", "quote", "bolt11", quote_id])?;

        let res = self.inner.get(url).send().await?.json::<Value>().await?;

        match serde_json::from_value::<MeltQuoteBolt11Response>(res.clone()) {
            Ok(melt_quote_response) => Ok(melt_quote_response),
            Err(_) => Err(ErrorResponse::from_value(res)?.into()),
        }
    }

    /// Melt [NUT-05]
    /// [Nut-08] Lightning fee return if outputs defined
    #[instrument(skip(self, request), fields(mint_url = %mint_url))]
    async fn post_melt(
        &self,
        mint_url: MintUrl,
        request: MeltBolt11Request,
    ) -> Result<MeltQuoteBolt11Response, Error> {
        let url = mint_url.join_paths(&["v1", "melt", "bolt11"])?;

        let res = self
            .inner
            .post(url)
            .json(&request)
            .send()
            .await?
            .json::<Value>()
            .await?;

        match serde_json::from_value::<MeltQuoteBolt11Response>(res.clone()) {
            Ok(melt_quote_response) => Ok(melt_quote_response),
            Err(_) => {
                if let Ok(res) = serde_json::from_value::<MeltQuoteBolt11Response>(res.clone()) {
                    return Ok(res);
                }
                Err(ErrorResponse::from_value(res)?.into())
            }
        }
    }

    /// Swap Token [NUT-03]
    #[instrument(skip(self, swap_request), fields(mint_url = %mint_url))]
    async fn post_swap(
        &self,
        mint_url: MintUrl,
        swap_request: SwapRequest,
    ) -> Result<SwapResponse, Error> {
        let url = mint_url.join_paths(&["v1", "swap"])?;

        let res = self
            .inner
            .post(url)
            .json(&swap_request)
            .send()
            .await?
            .json::<Value>()
            .await?;

        match serde_json::from_value::<SwapResponse>(res.clone()) {
            Ok(melt_quote_response) => Ok(melt_quote_response),
            Err(_) => Err(ErrorResponse::from_value(res)?.into()),
        }
    }

    /// Get Mint Info [NUT-06]
    #[instrument(skip(self), fields(mint_url = %mint_url))]
    async fn get_mint_info(&self, mint_url: MintUrl) -> Result<MintInfo, Error> {
        let url = mint_url.join_paths(&["v1", "info"])?;

        let res = self.inner.get(url).send().await?.json::<Value>().await?;

        match serde_json::from_value::<MintInfo>(res.clone()) {
            Ok(melt_quote_response) => Ok(melt_quote_response),
            Err(err) => {
                tracing::error!("Could not get mint info: {}", err);
                Err(ErrorResponse::from_value(res)?.into())
            }
        }
    }

    /// Spendable check [NUT-07]
    #[instrument(skip(self, request), fields(mint_url = %mint_url))]
    async fn post_check_state(
        &self,
        mint_url: MintUrl,
        request: CheckStateRequest,
    ) -> Result<CheckStateResponse, Error> {
        let url = mint_url.join_paths(&["v1", "checkstate"])?;

        let res = self
            .inner
            .post(url)
            .json(&request)
            .send()
            .await?
            .json::<Value>()
            .await?;

        match serde_json::from_value::<CheckStateResponse>(res.clone()) {
            Ok(melt_quote_response) => Ok(melt_quote_response),
            Err(_) => Err(ErrorResponse::from_value(res)?.into()),
        }
    }

    /// Restore request [NUT-13]
    #[instrument(skip(self, request), fields(mint_url = %mint_url))]
    async fn post_restore(
        &self,
        mint_url: MintUrl,
        request: RestoreRequest,
    ) -> Result<RestoreResponse, Error> {
        let url = mint_url.join_paths(&["v1", "restore"])?;

        let res = self
            .inner
            .post(url)
            .json(&request)
            .send()
            .await?
            .json::<Value>()
            .await?;

        match serde_json::from_value::<RestoreResponse>(res.clone()) {
            Ok(melt_quote_response) => Ok(melt_quote_response),
            Err(_) => Err(ErrorResponse::from_value(res)?.into()),
        }
    }
}

/// Http Client Methods
#[async_trait]
pub trait HttpClientMethods: Debug {
    /// Get Active Mint Keys [NUT-01]
    async fn get_mint_keys(&self, mint_url: MintUrl) -> Result<Vec<KeySet>, Error>;

    /// Get Keyset Keys [NUT-01]
    async fn get_mint_keyset(&self, mint_url: MintUrl, keyset_id: Id) -> Result<KeySet, Error>;

    /// Get Keysets [NUT-02]
    async fn get_mint_keysets(&self, mint_url: MintUrl) -> Result<KeysetResponse, Error>;

    /// Mint Quote [NUT-04]
    async fn post_mint_quote(
        &self,
        mint_url: MintUrl,
        request: MintQuoteBolt11Request,
    ) -> Result<MintQuoteBolt11Response, Error>;

    /// Mint Quote status
    async fn get_mint_quote_status(
        &self,
        mint_url: MintUrl,
        quote_id: &str,
    ) -> Result<MintQuoteBolt11Response, Error>;

    /// Mint Tokens [NUT-04]
    async fn post_mint(
        &self,
        mint_url: MintUrl,
        request: MintBolt11Request,
    ) -> Result<MintBolt11Response, Error>;

    /// Melt Quote [NUT-05]
    async fn post_melt_quote(
        &self,
        mint_url: MintUrl,
        request: MeltQuoteBolt11Request,
    ) -> Result<MeltQuoteBolt11Response, Error>;

    /// Melt Quote Status
    async fn get_melt_quote_status(
        &self,
        mint_url: MintUrl,
        quote_id: &str,
    ) -> Result<MeltQuoteBolt11Response, Error>;

    /// Melt [NUT-05]
    /// [Nut-08] Lightning fee return if outputs defined
    async fn post_melt(
        &self,
        mint_url: MintUrl,
        request: MeltBolt11Request,
    ) -> Result<MeltQuoteBolt11Response, Error>;

    /// Split Token [NUT-06]
    async fn post_swap(
        &self,
        mint_url: MintUrl,
        request: SwapRequest,
    ) -> Result<SwapResponse, Error>;

    /// Get Mint Info [NUT-06]
    async fn get_mint_info(&self, mint_url: MintUrl) -> Result<MintInfo, Error>;

    /// Spendable check [NUT-07]
    async fn post_check_state(
        &self,
        mint_url: MintUrl,
        request: CheckStateRequest,
    ) -> Result<CheckStateResponse, Error>;

    /// Restore request [NUT-13]
    async fn post_restore(
        &self,
        mint_url: MintUrl,
        request: RestoreRequest,
    ) -> Result<RestoreResponse, Error>;
}

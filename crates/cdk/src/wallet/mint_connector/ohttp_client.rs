//! OHTTP Mint client implementation
use std::collections::HashSet;
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use cdk_common::{nut19, MeltQuoteBolt12Request, MintQuoteBolt12Request, MintQuoteBolt12Response};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tracing::instrument;
use url::Url;

use super::ohttp_transport::OhttpTransport;
use super::transport::Transport;
use super::{Error, MintConnector};
use crate::mint_url::MintUrl;
use crate::nuts::{
    CheckStateRequest, CheckStateResponse, Id, KeySet, KeysResponse, KeysetResponse,
    MeltQuoteBolt11Request, MeltQuoteBolt11Response, MeltRequest, MintInfo, MintQuoteBolt11Request,
    MintQuoteBolt11Response, MintRequest, MintResponse, RestoreRequest, RestoreResponse,
    SwapRequest, SwapResponse,
};
#[cfg(feature = "auth")]
use crate::wallet::AuthWallet;

type Cache = (u64, HashSet<(nut19::Method, nut19::Path)>);

/// OHTTP Mint Client
#[derive(Debug, Clone)]
pub struct OhttpClient {
    transport: Arc<OhttpTransport>,
    mint_url: MintUrl,
    cache_support: Arc<StdRwLock<Cache>>,
}

impl OhttpClient {
    /// Create new OHTTP client with gateway and relay URLs
    ///
    /// The OHTTP request flow:
    /// 1. Request is sent to the relay URL
    /// 2. Relay forwards it to the gateway URL
    /// 3. Gateway forwards it to the target (mint) URL
    ///
    /// The mint URL is used as both the target URL and for fetching keys
    pub fn new(mint_url: MintUrl, gateway_url: Url, relay_url: Url) -> Self {
        // Use mint URL as target and keys source since the mint serves both roles
        let target_url = mint_url
            .join_paths(&[])
            .expect("Failed to create target URL");
        let keys_source_url = target_url.clone();

        let transport = OhttpTransport::new(target_url, gateway_url, relay_url, keys_source_url);

        Self {
            transport: Arc::new(transport),
            mint_url,
            cache_support: Default::default(),
        }
    }

    /// Generic implementation of a retriable http request
    ///
    /// The retry only happens if the mint supports replay through the Caching of NUT-19.
    #[inline(always)]
    async fn retriable_http_request<P, R>(
        &self,
        method: nut19::Method,
        path: nut19::Path,
        payload: &P,
    ) -> Result<R, Error>
    where
        P: Serialize + ?Sized + Send + Sync,
        R: DeserializeOwned,
    {
        let started = Instant::now();

        let retriable_window = self
            .cache_support
            .read()
            .map(|cache_support| {
                cache_support
                    .1
                    .get(&(method, path))
                    .map(|_| cache_support.0)
            })
            .unwrap_or_default()
            .map(Duration::from_secs)
            .unwrap_or_default();

        loop {
            let url = self.mint_url.join_paths(&match path {
                nut19::Path::MintBolt11 => vec!["v1", "mint", "bolt11"],
                nut19::Path::MeltBolt11 => vec!["v1", "melt", "bolt11"],
                nut19::Path::MintBolt12 => vec!["v1", "mint", "bolt12"],
                nut19::Path::MeltBolt12 => vec!["v1", "melt", "bolt12"],
                nut19::Path::Swap => vec!["v1", "swap"],
            })?;

            let result = match method {
                nut19::Method::Get => self.transport.http_get(url, None).await,
                nut19::Method::Post => self.transport.http_post(url, None, payload).await,
            };

            if result.is_ok() {
                return result;
            }

            match result.as_ref() {
                Err(Error::HttpError(status_code, _)) => {
                    let status_code = status_code.to_owned().unwrap_or_default();
                    if (400..=499).contains(&status_code) {
                        // 4xx errors won't be 'solved' by retrying
                        return result;
                    }

                    // retry request, if possible
                    tracing::error!("Failed OHTTP request {:?}", result.as_ref().err());

                    if retriable_window < started.elapsed() {
                        return result;
                    }
                }
                Err(_) => return result,
                _ => unreachable!(),
            };
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl MintConnector for OhttpClient {
    /// Get Active Mint Keys [NUT-01]
    #[instrument(skip(self), fields(mint_url = %self.mint_url))]
    async fn get_mint_keys(&self) -> Result<Vec<KeySet>, Error> {
        let url = self.mint_url.join_paths(&["v1", "keys"])?;
        Ok(self
            .transport
            .http_get::<KeysResponse>(url, None)
            .await?
            .keysets)
    }

    /// Get Keyset Keys [NUT-01]
    #[instrument(skip(self), fields(mint_url = %self.mint_url))]
    async fn get_mint_keyset(&self, keyset_id: Id) -> Result<KeySet, Error> {
        let url = self
            .mint_url
            .join_paths(&["v1", "keys", &keyset_id.to_string()])?;
        let keys_response = self.transport.http_get::<KeysResponse>(url, None).await?;
        Ok(keys_response.keysets.first().unwrap().clone())
    }

    /// Get Keysets [NUT-02]
    #[instrument(skip(self), fields(mint_url = %self.mint_url))]
    async fn get_mint_keysets(&self) -> Result<KeysetResponse, Error> {
        let url = self.mint_url.join_paths(&["v1", "keysets"])?;
        self.transport.http_get(url, None).await
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
        self.transport.http_post(url, None, &request).await
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
        self.transport.http_get(url, None).await
    }

    /// Mint Tokens [NUT-04]
    #[instrument(skip(self, request), fields(mint_url = %self.mint_url))]
    async fn post_mint(&self, request: MintRequest<String>) -> Result<MintResponse, Error> {
        self.retriable_http_request(nut19::Method::Post, nut19::Path::MintBolt11, &request)
            .await
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
        self.transport.http_post(url, None, &request).await
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
        self.transport.http_get(url, None).await
    }

    /// Melt [NUT-05]
    /// [Nut-08] Lightning fee return if outputs defined
    #[instrument(skip(self, request), fields(mint_url = %self.mint_url))]
    async fn post_melt(
        &self,
        request: MeltRequest<String>,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        self.retriable_http_request(nut19::Method::Post, nut19::Path::MeltBolt11, &request)
            .await
    }

    /// Swap Token [NUT-03]
    #[instrument(skip(self, swap_request), fields(mint_url = %self.mint_url))]
    async fn post_swap(&self, swap_request: SwapRequest) -> Result<SwapResponse, Error> {
        self.retriable_http_request(nut19::Method::Post, nut19::Path::Swap, &swap_request)
            .await
    }

    /// Helper to get mint info
    async fn get_mint_info(&self) -> Result<MintInfo, Error> {
        let url = self.mint_url.join_paths(&["v1", "info"])?;
        let info: MintInfo = self.transport.http_get(url, None).await?;

        if let Ok(mut cache_support) = self.cache_support.write() {
            *cache_support = (
                info.nuts.nut19.ttl.unwrap_or(300),
                info.nuts
                    .nut19
                    .cached_endpoints
                    .clone()
                    .into_iter()
                    .map(|cached_endpoint| (cached_endpoint.method, cached_endpoint.path))
                    .collect(),
            );
        }

        Ok(info)
    }

    /// Get the auth wallet for the client (not supported in OHTTP)
    #[cfg(feature = "auth")]
    async fn get_auth_wallet(&self) -> Option<AuthWallet> {
        None
    }

    /// Set auth wallet on client (not supported in OHTTP)
    #[cfg(feature = "auth")]
    async fn set_auth_wallet(&self, _wallet: Option<AuthWallet>) {
        // OHTTP client does not support auth
    }

    /// Spendable check [NUT-07]
    #[instrument(skip(self, request), fields(mint_url = %self.mint_url))]
    async fn post_check_state(
        &self,
        request: CheckStateRequest,
    ) -> Result<CheckStateResponse, Error> {
        let url = self.mint_url.join_paths(&["v1", "checkstate"])?;
        self.transport.http_post(url, None, &request).await
    }

    /// Restore request [NUT-13]
    #[instrument(skip(self, request), fields(mint_url = %self.mint_url))]
    async fn post_restore(&self, request: RestoreRequest) -> Result<RestoreResponse, Error> {
        let url = self.mint_url.join_paths(&["v1", "restore"])?;
        self.transport.http_post(url, None, &request).await
    }

    /// Mint Quote Bolt12 [NUT-23]
    #[instrument(skip(self), fields(mint_url = %self.mint_url))]
    async fn post_mint_bolt12_quote(
        &self,
        request: MintQuoteBolt12Request,
    ) -> Result<MintQuoteBolt12Response<String>, Error> {
        let url = self
            .mint_url
            .join_paths(&["v1", "mint", "quote", "bolt12"])?;
        self.transport.http_post(url, None, &request).await
    }

    /// Mint Quote Bolt12 status
    #[instrument(skip(self), fields(mint_url = %self.mint_url))]
    async fn get_mint_quote_bolt12_status(
        &self,
        quote_id: &str,
    ) -> Result<MintQuoteBolt12Response<String>, Error> {
        let url = self
            .mint_url
            .join_paths(&["v1", "mint", "quote", "bolt12", quote_id])?;
        self.transport.http_get(url, None).await
    }

    /// Melt Quote Bolt12 [NUT-23]
    #[instrument(skip(self, request), fields(mint_url = %self.mint_url))]
    async fn post_melt_bolt12_quote(
        &self,
        request: MeltQuoteBolt12Request,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        let url = self
            .mint_url
            .join_paths(&["v1", "melt", "quote", "bolt12"])?;
        self.transport.http_post(url, None, &request).await
    }

    /// Melt Quote Bolt12 Status [NUT-23]
    #[instrument(skip(self), fields(mint_url = %self.mint_url))]
    async fn get_melt_bolt12_quote_status(
        &self,
        quote_id: &str,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        let url = self
            .mint_url
            .join_paths(&["v1", "melt", "quote", "bolt12", quote_id])?;
        self.transport.http_get(url, None).await
    }

    /// Melt Bolt12 [NUT-23]
    #[instrument(skip(self, request), fields(mint_url = %self.mint_url))]
    async fn post_melt_bolt12(
        &self,
        request: MeltRequest<String>,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        self.retriable_http_request(nut19::Method::Post, nut19::Path::MeltBolt12, &request)
            .await
    }
}

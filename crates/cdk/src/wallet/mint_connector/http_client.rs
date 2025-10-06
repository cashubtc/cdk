//! HTTP Mint client with pluggable transport
use std::collections::HashSet;
use std::sync::{Arc, RwLock as StdRwLock};

use async_trait::async_trait;
use cdk_common::{nut19, MeltQuoteBolt12Request, MintQuoteBolt12Request, MintQuoteBolt12Response};
#[cfg(feature = "auth")]
use cdk_common::{Method, ProtectedEndpoint, RoutePath};
use serde::de::DeserializeOwned;
use serde::Serialize;
#[cfg(feature = "auth")]
use tokio::sync::RwLock;
use tracing::instrument;
use url::Url;
use web_time::{Duration, Instant};

use super::transport::Transport;
use super::{Error, MintConnector};
use crate::mint_url::MintUrl;
#[cfg(feature = "auth")]
use crate::nuts::nut22::MintAuthRequest;
use crate::nuts::{
    AuthToken, CheckStateRequest, CheckStateResponse, Id, KeySet, KeysResponse, KeysetResponse,
    MeltQuoteBolt11Request, MeltQuoteBolt11Response, MeltRequest, MintInfo, MintQuoteBolt11Request,
    MintQuoteBolt11Response, MintRequest, MintResponse, RestoreRequest, RestoreResponse,
    SwapRequest, SwapResponse,
};
#[cfg(feature = "auth")]
use crate::wallet::auth::{AuthMintConnector, AuthWallet};

type Cache = (u64, HashSet<(nut19::Method, nut19::Path)>);

/// Http Client
#[derive(Debug, Clone)]
pub struct HttpClient<T>
where
    T: Transport + Send + Sync + 'static,
{
    transport: Arc<T>,
    mint_url: MintUrl,
    cache_support: Arc<StdRwLock<Cache>>,
    #[cfg(feature = "auth")]
    auth_wallet: Arc<RwLock<Option<AuthWallet>>>,
}

impl<T> HttpClient<T>
where
    T: Transport + Send + Sync + 'static,
{
    /// Create new [`HttpClient`] with a provided transport implementation.
    #[cfg(feature = "auth")]
    pub fn with_transport(
        mint_url: MintUrl,
        transport: T,
        auth_wallet: Option<AuthWallet>,
    ) -> Self {
        Self {
            transport: transport.into(),
            mint_url,
            auth_wallet: Arc::new(RwLock::new(auth_wallet)),
            cache_support: Default::default(),
        }
    }

    /// Create new [`HttpClient`] with a provided transport implementation.
    #[cfg(not(feature = "auth"))]
    pub fn with_transport(mint_url: MintUrl, transport: T) -> Self {
        Self {
            transport: transport.into(),
            mint_url,
            cache_support: Default::default(),
        }
    }

    /// Create new [`HttpClient`]
    #[cfg(feature = "auth")]
    pub fn new(mint_url: MintUrl, auth_wallet: Option<AuthWallet>) -> Self {
        Self {
            transport: T::default().into(),
            mint_url,
            auth_wallet: Arc::new(RwLock::new(auth_wallet)),
            cache_support: Default::default(),
        }
    }

    #[cfg(not(feature = "auth"))]
    /// Create new [`HttpClient`]
    pub fn new(mint_url: MintUrl) -> Self {
        Self {
            transport: T::default().into(),
            cache_support: Default::default(),
            mint_url,
        }
    }

    /// Get auth token for a protected endpoint
    #[cfg(feature = "auth")]
    #[instrument(skip(self))]
    pub async fn get_auth_token(
        &self,
        method: Method,
        path: RoutePath,
    ) -> Result<Option<AuthToken>, Error> {
        let auth_wallet = self.auth_wallet.read().await;
        match auth_wallet.as_ref() {
            Some(auth_wallet) => {
                let endpoint = ProtectedEndpoint::new(method, path);
                auth_wallet.get_auth_for_request(&endpoint).await
            }
            None => Ok(None),
        }
    }

    /// Create new [`HttpClient`] with a proxy for specific TLDs.
    /// Specifying `None` for `host_matcher` will use the proxy for all
    /// requests.
    pub fn with_proxy(
        mint_url: MintUrl,
        proxy: Url,
        host_matcher: Option<&str>,
        accept_invalid_certs: bool,
    ) -> Result<Self, Error> {
        let mut transport = T::default();
        transport.with_proxy(proxy, host_matcher, accept_invalid_certs)?;

        Ok(Self {
            transport: transport.into(),
            mint_url,
            #[cfg(feature = "auth")]
            auth_wallet: Arc::new(RwLock::new(None)),
            cache_support: Default::default(),
        })
    }

    /// Generic implementation of a retriable http request
    ///
    /// The retry only happens if the mint supports replay through the Caching of NUT-19.
    #[inline(always)]
    async fn retriable_http_request<P, R>(
        &self,
        method: nut19::Method,
        path: nut19::Path,
        auth_token: Option<AuthToken>,
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

        let transport = self.transport.clone();
        loop {
            let url = self.mint_url.join_paths(&match path {
                nut19::Path::MintBolt11 => vec!["v1", "mint", "bolt11"],
                nut19::Path::MeltBolt11 => vec!["v1", "melt", "bolt11"],
                nut19::Path::MintBolt12 => vec!["v1", "mint", "bolt12"],

                nut19::Path::MeltBolt12 => vec!["v1", "melt", "bolt12"],
                nut19::Path::Swap => vec!["v1", "swap"],
            })?;

            let result = match method {
                nut19::Method::Get => transport.http_get(url, auth_token.clone()).await,
                nut19::Method::Post => transport.http_post(url, auth_token.clone(), payload).await,
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
                    tracing::error!("Failed http_request {:?}", result.as_ref().err());

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
impl<T> MintConnector for HttpClient<T>
where
    T: Transport + Send + Sync + 'static,
{
    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    #[instrument(skip(self), fields(mint_url = %self.mint_url))]
    async fn resolve_dns_txt(&self, domain: &str) -> Result<Vec<String>, Error> {
        self.transport.resolve_dns_txt(domain).await
    }

    /// Get Active Mint Keys [NUT-01]
    #[instrument(skip(self), fields(mint_url = %self.mint_url))]
    async fn get_mint_keys(&self) -> Result<Vec<KeySet>, Error> {
        let url = self.mint_url.join_paths(&["v1", "keys"])?;
        let transport = self.transport.clone();

        Ok(transport.http_get::<KeysResponse>(url, None).await?.keysets)
    }

    /// Get Keyset Keys [NUT-01]
    #[instrument(skip(self), fields(mint_url = %self.mint_url))]
    async fn get_mint_keyset(&self, keyset_id: Id) -> Result<KeySet, Error> {
        let url = self
            .mint_url
            .join_paths(&["v1", "keys", &keyset_id.to_string()])?;

        let transport = self.transport.clone();
        let keys_response = transport.http_get::<KeysResponse>(url, None).await?;

        Ok(keys_response.keysets.first().unwrap().clone())
    }

    /// Get Keysets [NUT-02]
    #[instrument(skip(self), fields(mint_url = %self.mint_url))]
    async fn get_mint_keysets(&self) -> Result<KeysetResponse, Error> {
        let url = self.mint_url.join_paths(&["v1", "keysets"])?;
        let transport = self.transport.clone();
        transport.http_get(url, None).await
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

        #[cfg(feature = "auth")]
        let auth_token = self
            .get_auth_token(Method::Post, RoutePath::MintQuoteBolt11)
            .await?;

        #[cfg(not(feature = "auth"))]
        let auth_token = None;

        self.transport.http_post(url, auth_token, &request).await
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

        #[cfg(feature = "auth")]
        let auth_token = self
            .get_auth_token(Method::Get, RoutePath::MintQuoteBolt11)
            .await?;

        #[cfg(not(feature = "auth"))]
        let auth_token = None;
        self.transport.http_get(url, auth_token).await
    }

    /// Mint Tokens [NUT-04]
    #[instrument(skip(self, request), fields(mint_url = %self.mint_url))]
    async fn post_mint(&self, request: MintRequest<String>) -> Result<MintResponse, Error> {
        #[cfg(feature = "auth")]
        let auth_token = self
            .get_auth_token(Method::Post, RoutePath::MintBolt11)
            .await?;

        #[cfg(not(feature = "auth"))]
        let auth_token = None;
        self.retriable_http_request(
            nut19::Method::Post,
            nut19::Path::MintBolt11,
            auth_token,
            &request,
        )
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
        #[cfg(feature = "auth")]
        let auth_token = self
            .get_auth_token(Method::Post, RoutePath::MeltQuoteBolt11)
            .await?;

        #[cfg(not(feature = "auth"))]
        let auth_token = None;
        self.transport.http_post(url, auth_token, &request).await
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

        #[cfg(feature = "auth")]
        let auth_token = self
            .get_auth_token(Method::Get, RoutePath::MeltQuoteBolt11)
            .await?;

        #[cfg(not(feature = "auth"))]
        let auth_token = None;
        self.transport.http_get(url, auth_token).await
    }

    /// Melt [NUT-05]
    /// [Nut-08] Lightning fee return if outputs defined
    #[instrument(skip(self, request), fields(mint_url = %self.mint_url))]
    async fn post_melt(
        &self,
        request: MeltRequest<String>,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        #[cfg(feature = "auth")]
        let auth_token = self
            .get_auth_token(Method::Post, RoutePath::MeltBolt11)
            .await?;

        #[cfg(not(feature = "auth"))]
        let auth_token = None;

        self.retriable_http_request(
            nut19::Method::Post,
            nut19::Path::MeltBolt11,
            auth_token,
            &request,
        )
        .await
    }

    /// Swap Token [NUT-03]
    #[instrument(skip(self, swap_request), fields(mint_url = %self.mint_url))]
    async fn post_swap(&self, swap_request: SwapRequest) -> Result<SwapResponse, Error> {
        #[cfg(feature = "auth")]
        let auth_token = self.get_auth_token(Method::Post, RoutePath::Swap).await?;

        #[cfg(not(feature = "auth"))]
        let auth_token = None;

        self.retriable_http_request(
            nut19::Method::Post,
            nut19::Path::Swap,
            auth_token,
            &swap_request,
        )
        .await
    }

    /// Helper to get mint info
    async fn get_mint_info(&self) -> Result<MintInfo, Error> {
        let url = self.mint_url.join_paths(&["v1", "info"])?;
        let transport = self.transport.clone();
        let info: MintInfo = transport.http_get(url, None).await?;

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

    #[cfg(feature = "auth")]
    async fn get_auth_wallet(&self) -> Option<AuthWallet> {
        self.auth_wallet.read().await.clone()
    }

    #[cfg(feature = "auth")]
    async fn set_auth_wallet(&self, wallet: Option<AuthWallet>) {
        *self.auth_wallet.write().await = wallet;
    }

    /// Spendable check [NUT-07]
    #[instrument(skip(self, request), fields(mint_url = %self.mint_url))]
    async fn post_check_state(
        &self,
        request: CheckStateRequest,
    ) -> Result<CheckStateResponse, Error> {
        let url = self.mint_url.join_paths(&["v1", "checkstate"])?;
        #[cfg(feature = "auth")]
        let auth_token = self
            .get_auth_token(Method::Post, RoutePath::Checkstate)
            .await?;

        #[cfg(not(feature = "auth"))]
        let auth_token = None;
        self.transport.http_post(url, auth_token, &request).await
    }

    /// Restore request [NUT-13]
    #[instrument(skip(self, request), fields(mint_url = %self.mint_url))]
    async fn post_restore(&self, request: RestoreRequest) -> Result<RestoreResponse, Error> {
        let url = self.mint_url.join_paths(&["v1", "restore"])?;
        #[cfg(feature = "auth")]
        let auth_token = self
            .get_auth_token(Method::Post, RoutePath::Restore)
            .await?;

        #[cfg(not(feature = "auth"))]
        let auth_token = None;
        self.transport.http_post(url, auth_token, &request).await
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

        #[cfg(feature = "auth")]
        let auth_token = self
            .get_auth_token(Method::Post, RoutePath::MintQuoteBolt12)
            .await?;

        #[cfg(not(feature = "auth"))]
        let auth_token = None;

        self.transport.http_post(url, auth_token, &request).await
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

        #[cfg(feature = "auth")]
        let auth_token = self
            .get_auth_token(Method::Get, RoutePath::MintQuoteBolt12)
            .await?;

        #[cfg(not(feature = "auth"))]
        let auth_token = None;
        self.transport.http_get(url, auth_token).await
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
        #[cfg(feature = "auth")]
        let auth_token = self
            .get_auth_token(Method::Post, RoutePath::MeltQuoteBolt12)
            .await?;

        #[cfg(not(feature = "auth"))]
        let auth_token = None;
        self.transport.http_post(url, auth_token, &request).await
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

        #[cfg(feature = "auth")]
        let auth_token = self
            .get_auth_token(Method::Get, RoutePath::MeltQuoteBolt12)
            .await?;

        #[cfg(not(feature = "auth"))]
        let auth_token = None;
        self.transport.http_get(url, auth_token).await
    }

    /// Melt Bolt12 [NUT-23]
    #[instrument(skip(self, request), fields(mint_url = %self.mint_url))]
    async fn post_melt_bolt12(
        &self,
        request: MeltRequest<String>,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        #[cfg(feature = "auth")]
        let auth_token = self
            .get_auth_token(Method::Post, RoutePath::MeltBolt12)
            .await?;

        #[cfg(not(feature = "auth"))]
        let auth_token = None;
        self.retriable_http_request(
            nut19::Method::Post,
            nut19::Path::MeltBolt12,
            auth_token,
            &request,
        )
        .await
    }
}

/// Http Client
#[derive(Debug, Clone)]
#[cfg(feature = "auth")]
pub struct AuthHttpClient<T>
where
    T: Transport + Send + Sync + 'static,
{
    transport: Arc<T>,
    mint_url: MintUrl,
    cat: Arc<RwLock<AuthToken>>,
}

#[cfg(feature = "auth")]
impl<T> AuthHttpClient<T>
where
    T: Transport + Send + Sync + 'static,
{
    /// Create new [`AuthHttpClient`]
    pub fn new(mint_url: MintUrl, cat: Option<AuthToken>) -> Self {
        Self {
            transport: T::default().into(),
            mint_url,
            cat: Arc::new(RwLock::new(
                cat.unwrap_or(AuthToken::ClearAuth("".to_string())),
            )),
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg(feature = "auth")]
impl<T> AuthMintConnector for AuthHttpClient<T>
where
    T: Transport + Send + Sync + 'static,
{
    async fn get_auth_token(&self) -> Result<AuthToken, Error> {
        Ok(self.cat.read().await.clone())
    }

    async fn set_auth_token(&self, token: AuthToken) -> Result<(), Error> {
        *self.cat.write().await = token;
        Ok(())
    }

    /// Get Mint Info [NUT-06]
    async fn get_mint_info(&self) -> Result<MintInfo, Error> {
        let url = self.mint_url.join_paths(&["v1", "info"])?;
        let mint_info: MintInfo = self.transport.http_get::<MintInfo>(url, None).await?;

        Ok(mint_info)
    }

    /// Get Auth Keyset Keys [NUT-22]
    #[instrument(skip(self), fields(mint_url = %self.mint_url))]
    async fn get_mint_blind_auth_keyset(&self, keyset_id: Id) -> Result<KeySet, Error> {
        let url =
            self.mint_url
                .join_paths(&["v1", "auth", "blind", "keys", &keyset_id.to_string()])?;

        let mut keys_response = self.transport.http_get::<KeysResponse>(url, None).await?;

        let keyset = keys_response
            .keysets
            .drain(0..1)
            .next()
            .ok_or_else(|| Error::UnknownKeySet)?;

        Ok(keyset)
    }

    /// Get Auth Keysets [NUT-22]
    #[instrument(skip(self), fields(mint_url = %self.mint_url))]
    async fn get_mint_blind_auth_keysets(&self) -> Result<KeysetResponse, Error> {
        let url = self
            .mint_url
            .join_paths(&["v1", "auth", "blind", "keysets"])?;

        self.transport.http_get(url, None).await
    }

    /// Mint Tokens [NUT-22]
    #[instrument(skip(self, request), fields(mint_url = %self.mint_url))]
    async fn post_mint_blind_auth(&self, request: MintAuthRequest) -> Result<MintResponse, Error> {
        let url = self.mint_url.join_paths(&["v1", "auth", "blind", "mint"])?;
        self.transport
            .http_post(url, Some(self.cat.read().await.clone()), &request)
            .await
    }
}

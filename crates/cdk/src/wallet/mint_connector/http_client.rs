use std::collections::HashSet;
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use cdk_common::{
    nut19, MeltQuoteBolt12Request, MeltQuoteOnchainRequest, MeltQuoteOnchainResponse,
    MintQuoteBolt12Request, MintQuoteBolt12Response, MintQuoteOnchainRequest,
    MintQuoteOnchainResponse,
};
#[cfg(feature = "auth")]
use cdk_common::{Method, ProtectedEndpoint, RoutePath};
use reqwest::{Client, IntoUrl};
use serde::de::DeserializeOwned;
use serde::Serialize;
#[cfg(feature = "auth")]
use tokio::sync::RwLock;
use tracing::instrument;
#[cfg(not(target_arch = "wasm32"))]
use url::Url;

use super::{Error, MintConnector};
use crate::error::ErrorResponse;
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

#[derive(Debug, Clone)]
struct HttpClientCore {
    inner: Client,
}

impl HttpClientCore {
    fn new() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }

        Self {
            inner: Client::new(),
        }
    }

    fn client(&self) -> &Client {
        &self.inner
    }

    async fn http_get<U: IntoUrl + Send, R: DeserializeOwned>(
        &self,
        url: U,
        auth: Option<AuthToken>,
    ) -> Result<R, Error> {
        let mut request = self.client().get(url);

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

    async fn http_post<U: IntoUrl + Send, P: Serialize + ?Sized, R: DeserializeOwned>(
        &self,
        url: U,
        auth_token: Option<AuthToken>,
        payload: &P,
    ) -> Result<R, Error> {
        let mut request = self.client().post(url).json(&payload);

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

type Cache = (u64, HashSet<(nut19::Method, nut19::Path)>);

/// Http Client
#[derive(Debug, Clone)]
pub struct HttpClient {
    core: HttpClientCore,
    mint_url: MintUrl,
    cache_support: Arc<StdRwLock<Cache>>,
    #[cfg(feature = "auth")]
    auth_wallet: Arc<RwLock<Option<AuthWallet>>>,
}

impl HttpClient {
    /// Create new [`HttpClient`]
    #[cfg(feature = "auth")]
    pub fn new(mint_url: MintUrl, auth_wallet: Option<AuthWallet>) -> Self {
        Self {
            core: HttpClientCore::new(),
            mint_url,
            auth_wallet: Arc::new(RwLock::new(auth_wallet)),
            cache_support: Default::default(),
        }
    }

    #[cfg(not(feature = "auth"))]
    /// Create new [`HttpClient`]
    pub fn new(mint_url: MintUrl) -> Self {
        Self {
            core: HttpClientCore::new(),
            cache_support: Default::default(),
            mint_url,
        }
    }

    /// Get auth token for a protected endpoint
    #[cfg(feature = "auth")]
    #[instrument(skip(self))]
    async fn get_auth_token(
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
            .map_err(|e| {
                Error::HttpError(
                    e.status().map(|status_code| status_code.as_u16()),
                    e.to_string(),
                )
            })?;

        Ok(Self {
            core: HttpClientCore { inner: client },
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
        P: Serialize + ?Sized,
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
                nut19::Method::Get => self.core.http_get(url, auth_token.clone()).await,
                nut19::Method::Post => self.core.http_post(url, auth_token.clone(), payload).await,
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
impl MintConnector for HttpClient {
    /// Get Active Mint Keys [NUT-01]
    #[instrument(skip(self), fields(mint_url = %self.mint_url))]
    async fn get_mint_keys(&self) -> Result<Vec<KeySet>, Error> {
        let url = self.mint_url.join_paths(&["v1", "keys"])?;

        Ok(self
            .core
            .http_get::<_, KeysResponse>(url, None)
            .await?
            .keysets)
    }

    /// Get Keyset Keys [NUT-01]
    #[instrument(skip(self), fields(mint_url = %self.mint_url))]
    async fn get_mint_keyset(&self, keyset_id: Id) -> Result<KeySet, Error> {
        let url = self
            .mint_url
            .join_paths(&["v1", "keys", &keyset_id.to_string()])?;

        let keys_response = self.core.http_get::<_, KeysResponse>(url, None).await?;

        Ok(keys_response.keysets.first().unwrap().clone())
    }

    /// Get Keysets [NUT-02]
    #[instrument(skip(self), fields(mint_url = %self.mint_url))]
    async fn get_mint_keysets(&self) -> Result<KeysetResponse, Error> {
        let url = self.mint_url.join_paths(&["v1", "keysets"])?;
        self.core.http_get(url, None).await
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

        self.core.http_post(url, auth_token, &request).await
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
        self.core.http_get(url, auth_token).await
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
        self.core.http_post(url, auth_token, &request).await
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
        self.core.http_get(url, auth_token).await
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
        let info: MintInfo = self.core.http_get(url, None).await?;

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
        self.core.http_post(url, auth_token, &request).await
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
        self.core.http_post(url, auth_token, &request).await
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

        self.core.http_post(url, auth_token, &request).await
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
        self.core.http_get(url, auth_token).await
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
        self.core.http_post(url, auth_token, &request).await
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
        self.core.http_get(url, auth_token).await
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

    /// Mint Quote Onchain [NUT-26]
    #[instrument(skip(self), fields(mint_url = %self.mint_url))]
    async fn post_mint_onchain_quote(
        &self,
        request: MintQuoteOnchainRequest,
    ) -> Result<MintQuoteOnchainResponse<String>, Error> {
        let url = self
            .mint_url
            .join_paths(&["v1", "mint", "quote", "onchain"])?;

        #[cfg(feature = "auth")]
        let auth_token = self
            .get_auth_token(Method::Post, RoutePath::MintQuoteOnchain)
            .await?;

        #[cfg(not(feature = "auth"))]
        let auth_token = None;

        self.core.http_post(url, auth_token, &request).await
    }

    /// Mint Quote Onchain status [NUT-26]
    #[instrument(skip(self), fields(mint_url = %self.mint_url))]
    async fn get_mint_quote_onchain_status(
        &self,
        quote_id: &str,
    ) -> Result<MintQuoteOnchainResponse<String>, Error> {
        let url = self
            .mint_url
            .join_paths(&["v1", "mint", "quote", "onchain", quote_id])?;

        #[cfg(feature = "auth")]
        let auth_token = self
            .get_auth_token(Method::Get, RoutePath::MintQuoteOnchain)
            .await?;

        #[cfg(not(feature = "auth"))]
        let auth_token = None;
        self.core.http_get(url, auth_token).await
    }

    /// Melt Quote Onchain [NUT-26]
    #[instrument(skip(self, request), fields(mint_url = %self.mint_url))]
    async fn post_melt_onchain_quote(
        &self,
        request: MeltQuoteOnchainRequest,
    ) -> Result<MeltQuoteOnchainResponse<String>, Error> {
        let url = self
            .mint_url
            .join_paths(&["v1", "melt", "quote", "onchain"])?;
        #[cfg(feature = "auth")]
        let auth_token = self
            .get_auth_token(Method::Post, RoutePath::MeltQuoteOnchain)
            .await?;

        #[cfg(not(feature = "auth"))]
        let auth_token = None;
        self.core.http_post(url, auth_token, &request).await
    }

    /// Melt Quote Onchain Status [NUT-26]
    #[instrument(skip(self), fields(mint_url = %self.mint_url))]
    async fn get_melt_onchain_quote_status(
        &self,
        quote_id: &str,
    ) -> Result<MeltQuoteOnchainResponse<String>, Error> {
        let url = self
            .mint_url
            .join_paths(&["v1", "melt", "quote", "onchain", quote_id])?;

        #[cfg(feature = "auth")]
        let auth_token = self
            .get_auth_token(Method::Get, RoutePath::MeltQuoteOnchain)
            .await?;

        #[cfg(not(feature = "auth"))]
        let auth_token = None;
        self.core.http_get(url, auth_token).await
    }

    /// Melt Onchain [NUT-26]
    #[instrument(skip(self, request), fields(mint_url = %self.mint_url))]
    async fn post_melt_onchain(
        &self,
        request: MeltRequest<String>,
    ) -> Result<MeltQuoteOnchainResponse<String>, Error> {
        let url = self.mint_url.join_paths(&["v1", "melt", "onchain"])?;
        #[cfg(feature = "auth")]
        let auth_token = self
            .get_auth_token(Method::Post, RoutePath::MeltOnchain)
            .await?;

        #[cfg(not(feature = "auth"))]
        let auth_token = None;
        self.core.http_post(url, auth_token, &request).await
    }
}

/// Http Client
#[derive(Debug, Clone)]
#[cfg(feature = "auth")]
pub struct AuthHttpClient {
    core: HttpClientCore,
    mint_url: MintUrl,
    cat: Arc<RwLock<AuthToken>>,
}

#[cfg(feature = "auth")]
impl AuthHttpClient {
    /// Create new [`AuthHttpClient`]
    pub fn new(mint_url: MintUrl, cat: Option<AuthToken>) -> Self {
        Self {
            core: HttpClientCore::new(),
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
impl AuthMintConnector for AuthHttpClient {
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
        let mint_info: MintInfo = self.core.http_get::<_, MintInfo>(url, None).await?;

        Ok(mint_info)
    }

    /// Get Auth Keyset Keys [NUT-22]
    #[instrument(skip(self), fields(mint_url = %self.mint_url))]
    async fn get_mint_blind_auth_keyset(&self, keyset_id: Id) -> Result<KeySet, Error> {
        let url =
            self.mint_url
                .join_paths(&["v1", "auth", "blind", "keys", &keyset_id.to_string()])?;

        let mut keys_response = self.core.http_get::<_, KeysResponse>(url, None).await?;

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

        self.core.http_get(url, None).await
    }

    /// Mint Tokens [NUT-22]
    #[instrument(skip(self, request), fields(mint_url = %self.mint_url))]
    async fn post_mint_blind_auth(&self, request: MintAuthRequest) -> Result<MintResponse, Error> {
        let url = self.mint_url.join_paths(&["v1", "auth", "blind", "mint"])?;
        self.core
            .http_post(url, Some(self.cat.read().await.clone()), &request)
            .await
    }
}

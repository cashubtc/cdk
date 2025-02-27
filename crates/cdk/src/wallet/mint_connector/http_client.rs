use async_trait::async_trait;
use cdk_common::{Method, ProtectedEndpoint, RoutePath};
use reqwest::{Client, IntoUrl};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tracing::instrument;
#[cfg(not(target_arch = "wasm32"))]
use url::Url;

use super::{Error, MintConnector};
use crate::error::ErrorResponse;
use crate::mint_url::MintUrl;
use crate::nuts::nut22::MintAuthRequest;
use crate::nuts::{
    AuthToken, CheckStateRequest, CheckStateResponse, Id, KeySet, KeysResponse, KeysetResponse,
    MeltBolt11Request, MeltQuoteBolt11Request, MeltQuoteBolt11Response, MintBolt11Request,
    MintBolt11Response, MintInfo, MintQuoteBolt11Request, MintQuoteBolt11Response, RestoreRequest,
    RestoreResponse, SwapRequest, SwapResponse,
};
use crate::wallet::auth::{AuthMintConnector, AuthWallet};

#[derive(Debug, Clone)]
struct HttpClientCore {
    inner: Client,
}

impl HttpClientCore {
    fn new() -> Self {
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
            .map_err(|e| Error::HttpError(e.to_string()))?
            .text()
            .await
            .map_err(|e| Error::HttpError(e.to_string()))?;

        println!("{}", response);

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

        let response = request
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
}

/// Http Client
#[derive(Debug, Clone)]
pub struct HttpClient {
    core: HttpClientCore,
    mint_url: MintUrl,
    auth_wallet: Option<AuthWallet>,
}

impl HttpClient {
    /// Create new [`HttpClient`]
    pub fn new(mint_url: MintUrl, auth_wallet: Option<AuthWallet>) -> Self {
        Self {
            core: HttpClientCore::new(),
            mint_url,
            auth_wallet,
        }
    }

    /// Get auth token for a protected endpoint
    async fn get_auth_token(
        &self,
        method: Method,
        path: RoutePath,
    ) -> Result<Option<AuthToken>, Error> {
        match self.auth_wallet.as_ref() {
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
            .map_err(|e| Error::HttpError(e.to_string()))?;

        Ok(Self {
            core: HttpClientCore { inner: client },
            mint_url,
            auth_wallet: None,
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

        let auth_token = self
            .get_auth_token(Method::Post, RoutePath::MintQuoteBolt11)
            .await?;

        println!("{:?}", auth_token);

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

        let auth_token = self
            .get_auth_token(Method::Get, RoutePath::MintQuoteBolt11)
            .await?;
        self.core.http_get(url, auth_token).await
    }

    /// Mint Tokens [NUT-04]
    #[instrument(skip(self, request), fields(mint_url = %self.mint_url))]
    async fn post_mint(
        &self,
        request: MintBolt11Request<String>,
    ) -> Result<MintBolt11Response, Error> {
        let url = self.mint_url.join_paths(&["v1", "mint", "bolt11"])?;
        let auth_token = self
            .get_auth_token(Method::Post, RoutePath::MintBolt11)
            .await?;
        self.core.http_post(url, auth_token, &request).await
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
        let auth_token = self
            .get_auth_token(Method::Post, RoutePath::MeltQuoteBolt11)
            .await?;
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

        let auth_token = self
            .get_auth_token(Method::Get, RoutePath::MeltQuoteBolt11)
            .await?;
        self.core.http_get(url, auth_token).await
    }

    /// Melt [NUT-05]
    /// [Nut-08] Lightning fee return if outputs defined
    #[instrument(skip(self, request), fields(mint_url = %self.mint_url))]
    async fn post_melt(
        &self,
        request: MeltBolt11Request<String>,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        let url = self.mint_url.join_paths(&["v1", "melt", "bolt11"])?;
        let auth_token = self
            .get_auth_token(Method::Post, RoutePath::MeltBolt11)
            .await?;
        self.core.http_post(url, auth_token, &request).await
    }

    /// Swap Token [NUT-03]
    #[instrument(skip(self, swap_request), fields(mint_url = %self.mint_url))]
    async fn post_swap(&self, swap_request: SwapRequest) -> Result<SwapResponse, Error> {
        let url = self.mint_url.join_paths(&["v1", "swap"])?;
        let auth_token = self.get_auth_token(Method::Post, RoutePath::Swap).await?;
        self.core.http_post(url, auth_token, &swap_request).await
    }

    /// Helper to get mint info and update protected endpoints
    async fn get_mint_info(&self) -> Result<MintInfo, Error> {
        let url = self.mint_url.join_paths(&["v1", "info"])?;
        let mint_info: MintInfo = self.core.http_get::<_, MintInfo>(url, None).await?;

        if let Some(auth_wallet) = self.auth_wallet.as_ref() {
            let mut protected_endpoints = auth_wallet.protected_endpoints.write().await;
            *protected_endpoints = mint_info.protected_endpoints();
        }

        Ok(mint_info)
    }

    /// Spendable check [NUT-07]
    #[instrument(skip(self, request), fields(mint_url = %self.mint_url))]
    async fn post_check_state(
        &self,
        request: CheckStateRequest,
    ) -> Result<CheckStateResponse, Error> {
        let url = self.mint_url.join_paths(&["v1", "checkstate"])?;
        let auth_token = self
            .get_auth_token(Method::Post, RoutePath::Checkstate)
            .await?;
        self.core.http_post(url, auth_token, &request).await
    }

    /// Restore request [NUT-13]
    #[instrument(skip(self, request), fields(mint_url = %self.mint_url))]
    async fn post_restore(&self, request: RestoreRequest) -> Result<RestoreResponse, Error> {
        let url = self.mint_url.join_paths(&["v1", "restore"])?;
        let auth_token = self
            .get_auth_token(Method::Post, RoutePath::Restore)
            .await?;
        self.core.http_post(url, auth_token, &request).await
    }
}

/// Http Client
#[derive(Debug, Clone)]
pub struct AuthHttpClient {
    core: HttpClientCore,
    mint_url: MintUrl,
    cat: AuthToken,
}

impl AuthHttpClient {
    /// Create new [`AuthHttpClient`]
    pub fn new(mint_url: MintUrl, cat: AuthToken) -> Self {
        Self {
            core: HttpClientCore::new(),
            mint_url,
            cat,
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AuthMintConnector for AuthHttpClient {
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
    async fn post_mint_blind_auth(
        &self,
        request: MintAuthRequest,
    ) -> Result<MintBolt11Response, Error> {
        let url = self.mint_url.join_paths(&["v1", "auth", "blind", "mint"])?;
        self.core
            .http_post(url, Some(self.cat.clone()), &request)
            .await
    }
}

//! HTTP Mint client with pluggable transport
use std::collections::HashSet;
use std::sync::{Arc, RwLock as StdRwLock};

use async_trait::async_trait;
use cdk_common::{
    nut19, MeltQuoteCreateResponse, MeltQuoteRequest, MeltQuoteResponse, Method,
    MintQuoteBolt11Response, MintQuoteBolt12Response, MintQuoteCustomResponse, MintQuoteRequest,
    MintQuoteResponse, MintQuoteState, ProtectedEndpoint, RoutePath,
};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::sync::RwLock;
use tracing::instrument;
use url::Url;
use web_time::{Duration, Instant};

use super::transport::Transport;
use super::{Error, MintConnector};
use crate::mint_url::MintUrl;
use crate::nuts::nut00::{KnownMethod, PaymentMethod};
use crate::nuts::nut22::MintAuthRequest;
use crate::nuts::{
    AuthToken, BatchCheckMintQuoteRequest, BatchMintRequest, CheckStateRequest, CheckStateResponse,
    Id, KeySet, KeysResponse, KeysetResponse, MeltRequest, MintInfo, MintRequest, MintResponse,
    RestoreRequest, RestoreResponse, SwapRequest, SwapResponse,
};
use crate::wallet::auth::{AuthMintConnector, AuthWallet};

type Cache = (u64, HashSet<(nut19::Method, nut19::Path)>);

fn take_custom_mint_quote_state(
    response: &mut MintQuoteCustomResponse<String>,
) -> Option<MintQuoteState> {
    let serde_json::Value::Object(fields) = &mut response.extra else {
        return None;
    };

    fields
        .remove("state")
        .and_then(|state| serde_json::from_value(state).ok())
}

/// Http Client
#[derive(Debug, Clone)]
pub struct HttpClient<T>
where
    T: Transport + Send + Sync + 'static,
{
    transport: Arc<T>,
    mint_url: MintUrl,
    cache_support: Arc<StdRwLock<Cache>>,
    auth_wallet: Arc<RwLock<Option<AuthWallet>>>,
}

impl<T> HttpClient<T>
where
    T: Transport + Send + Sync + 'static,
{
    /// Create new [`HttpClient`] with a provided transport implementation.
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

    /// Create new [`HttpClient`]
    pub fn new(mint_url: MintUrl, auth_wallet: Option<AuthWallet>) -> Self {
        Self {
            transport: T::default().into(),
            mint_url,
            auth_wallet: Arc::new(RwLock::new(auth_wallet)),
            cache_support: Default::default(),
        }
    }

    /// Get auth token for a protected endpoint
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
                    .get(&(method, path.clone()))
                    .map(|_| cache_support.0)
            })
            .unwrap_or_default()
            .map(Duration::from_secs)
            .unwrap_or_default();

        let transport = self.transport.clone();
        loop {
            let url = match &path {
                nut19::Path::Swap => self.mint_url.join_paths(&["v1", "swap"])?,
                nut19::Path::Custom(custom_path) => {
                    // Custom paths should be in the format "/v1/mint/{method}" or "/v1/melt/{method}"
                    // Remove leading slash if present
                    let path_str = custom_path.trim_start_matches('/');
                    let parts: Vec<&str> = path_str.split('/').collect();
                    self.mint_url.join_paths(&parts)?
                }
            };

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

    /// Fetch Lightning address pay request data
    #[instrument(skip(self))]
    async fn fetch_lnurl_pay_request(
        &self,
        url: &str,
    ) -> Result<crate::lightning_address::LnurlPayResponse, Error> {
        let parsed_url =
            url::Url::parse(url).map_err(|e| Error::Custom(format!("Invalid URL: {}", e)))?;
        self.transport.http_get(parsed_url, None).await
    }

    /// Fetch invoice from Lightning address callback
    #[instrument(skip(self))]
    async fn fetch_lnurl_invoice(
        &self,
        url: &str,
    ) -> Result<crate::lightning_address::LnurlPayInvoiceResponse, Error> {
        let parsed_url =
            url::Url::parse(url).map_err(|e| Error::Custom(format!("Invalid URL: {}", e)))?;
        self.transport.http_get(parsed_url, None).await
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

        Ok(keys_response
            .keysets
            .first()
            .ok_or(Error::UnknownKeySet)?
            .clone())
    }

    /// Get Keysets [NUT-02]
    #[instrument(skip(self), fields(mint_url = %self.mint_url))]
    async fn get_mint_keysets(&self) -> Result<KeysetResponse, Error> {
        let url = self.mint_url.join_paths(&["v1", "keysets"])?;
        let transport = self.transport.clone();
        transport.http_get(url, None).await
    }

    /// Mint Quote [NUT-04, NUT-23, NUT-25]
    #[instrument(skip(self, request), fields(mint_url = %self.mint_url))]
    async fn post_mint_quote(
        &self,
        request: MintQuoteRequest,
    ) -> Result<MintQuoteResponse<String>, Error> {
        let method = request.method().to_string();
        let path = format!("v1/mint/quote/{}", method);

        let url = self
            .mint_url
            .join_paths(&path.split('/').collect::<Vec<_>>())?;

        let auth_token = self
            .get_auth_token(
                Method::Post,
                RoutePath::MintQuote(request.method().to_string()),
            )
            .await?;

        match &request {
            MintQuoteRequest::Bolt11(req) => {
                let response: cdk_common::nut23::MintQuoteBolt11Response<String> =
                    self.transport.http_post(url, auth_token, req).await?;
                Ok(MintQuoteResponse::Bolt11(response))
            }
            MintQuoteRequest::Bolt12(req) => {
                let response: cdk_common::nut25::MintQuoteBolt12Response<String> =
                    self.transport.http_post(url, auth_token, req).await?;
                Ok(MintQuoteResponse::Bolt12(response))
            }
            MintQuoteRequest::Custom { request: req, .. } => {
                let mut response: cdk_common::nut04::MintQuoteCustomResponse<String> =
                    self.transport.http_post(url, auth_token, req).await?;
                let state =
                    take_custom_mint_quote_state(&mut response).or(Some(MintQuoteState::Unpaid));
                Ok(MintQuoteResponse::Custom {
                    method: request.method(),
                    state,
                    response,
                })
            }
        }
    }

    /// Mint Quote status with payment method
    #[instrument(skip(self), fields(mint_url = %self.mint_url))]
    async fn get_mint_quote_status(
        &self,
        method: PaymentMethod,
        quote_id: &str,
    ) -> Result<MintQuoteResponse<String>, Error> {
        match &method {
            PaymentMethod::Known(KnownMethod::Bolt11) => {
                let url = self
                    .mint_url
                    .join_paths(&["v1", "mint", "quote", "bolt11", quote_id])?;

                let auth_token = self
                    .get_auth_token(
                        Method::Get,
                        RoutePath::MintQuote(PaymentMethod::Known(KnownMethod::Bolt11).to_string()),
                    )
                    .await?;

                let response: MintQuoteBolt11Response<String> =
                    self.transport.http_get(url, auth_token).await?;

                Ok(MintQuoteResponse::Bolt11(response))
            }
            PaymentMethod::Known(KnownMethod::Bolt12) => {
                let url = self
                    .mint_url
                    .join_paths(&["v1", "mint", "quote", "bolt12", quote_id])?;

                let auth_token = self
                    .get_auth_token(
                        Method::Get,
                        RoutePath::MintQuote(PaymentMethod::Known(KnownMethod::Bolt12).to_string()),
                    )
                    .await?;

                let response: MintQuoteBolt12Response<String> =
                    self.transport.http_get(url, auth_token).await?;

                Ok(MintQuoteResponse::Bolt12(response))
            }
            PaymentMethod::Custom(method_name) => {
                let url =
                    self.mint_url
                        .join_paths(&["v1", "mint", "quote", method_name, quote_id])?;

                let auth_token = self
                    .get_auth_token(Method::Get, RoutePath::MintQuote(method_name.clone()))
                    .await?;

                let mut response: MintQuoteCustomResponse<String> =
                    self.transport.http_get(url, auth_token).await?;
                let state = take_custom_mint_quote_state(&mut response);

                Ok(MintQuoteResponse::Custom {
                    method,
                    state,
                    response,
                })
            }
        }
    }

    /// Mint Tokens [NUT-04]
    #[instrument(skip(self, request), fields(mint_url = %self.mint_url))]
    async fn post_mint(
        &self,
        method: &PaymentMethod,
        request: MintRequest<String>,
    ) -> Result<MintResponse, Error> {
        let auth_token = self
            .get_auth_token(Method::Post, RoutePath::Mint(method.to_string()))
            .await?;

        let path = match method {
            PaymentMethod::Known(KnownMethod::Bolt11) => {
                nut19::Path::Custom("/v1/mint/bolt11".to_string())
            }
            PaymentMethod::Known(KnownMethod::Bolt12) => {
                nut19::Path::Custom("/v1/mint/bolt12".to_string())
            }
            PaymentMethod::Custom(m) => nut19::Path::custom_mint(m),
        };

        self.retriable_http_request(nut19::Method::Post, path, auth_token, &request)
            .await
    }

    /// Batch check mint quote status [NUT-29]
    #[instrument(skip(self, request), fields(mint_url = %self.mint_url))]
    async fn post_batch_check_mint_quote_status(
        &self,
        method: &PaymentMethod,
        request: BatchCheckMintQuoteRequest<String>,
    ) -> Result<Vec<MintQuoteBolt11Response<String>>, Error> {
        let url =
            self.mint_url
                .join_paths(&["v1", "mint", "quote", &method.to_string(), "check"])?;

        let auth_token = self
            .get_auth_token(Method::Post, RoutePath::MintQuote(method.to_string()))
            .await?;

        self.transport.http_post(url, auth_token, &request).await
    }

    /// Batch mint tokens [NUT-29]
    #[instrument(skip(self, request), fields(mint_url = %self.mint_url))]
    async fn post_batch_mint(
        &self,
        method: &PaymentMethod,
        request: BatchMintRequest<String>,
    ) -> Result<MintResponse, Error> {
        let auth_token = self
            .get_auth_token(Method::Post, RoutePath::Mint(method.to_string()))
            .await?;

        let path = nut19::Path::Custom(format!("/v1/mint/{}/batch", method));

        self.retriable_http_request(nut19::Method::Post, path, auth_token, &request)
            .await
    }

    /// Melt Quote [NUT-05]
    #[instrument(skip(self, request), fields(mint_url = %self.mint_url))]
    async fn post_melt_quote(
        &self,
        request: MeltQuoteRequest,
    ) -> Result<MeltQuoteCreateResponse<String>, Error> {
        let method = request.method().to_string();
        let path = format!("v1/melt/quote/{}", method);

        let url = self
            .mint_url
            .join_paths(&path.split('/').collect::<Vec<_>>())?;
        let auth_token = self
            .get_auth_token(Method::Post, RoutePath::MeltQuote(method))
            .await?;

        match &request {
            MeltQuoteRequest::Bolt11(req) => {
                let response: cdk_common::nut23::MeltQuoteBolt11Response<String> =
                    self.transport.http_post(url, auth_token, req).await?;
                Ok(MeltQuoteCreateResponse::Bolt11(response))
            }
            MeltQuoteRequest::Bolt12(req) => {
                let response: cdk_common::nut25::MeltQuoteBolt12Response<String> =
                    self.transport.http_post(url, auth_token, req).await?;
                Ok(MeltQuoteCreateResponse::Bolt12(response))
            }
            MeltQuoteRequest::Custom(req) => {
                let response: cdk_common::nut05::MeltQuoteCustomResponse<String> =
                    self.transport.http_post(url, auth_token, req).await?;
                Ok(MeltQuoteCreateResponse::Custom((
                    request.method(),
                    response,
                )))
            }
        }
    }

    /// Melt Quote Status
    #[instrument(skip(self), fields(mint_url = %self.mint_url))]
    async fn get_melt_quote_status(
        &self,
        method: PaymentMethod,
        quote_id: &str,
    ) -> Result<MeltQuoteResponse<String>, Error> {
        match &method {
            PaymentMethod::Known(KnownMethod::Bolt11) => {
                let url = self
                    .mint_url
                    .join_paths(&["v1", "melt", "quote", "bolt11", quote_id])?;

                let auth_token = self
                    .get_auth_token(
                        Method::Get,
                        RoutePath::MeltQuote(PaymentMethod::Known(KnownMethod::Bolt11).to_string()),
                    )
                    .await?;

                let response: cdk_common::nut23::MeltQuoteBolt11Response<String> =
                    self.transport.http_get(url, auth_token).await?;

                Ok(MeltQuoteResponse::Bolt11(response))
            }
            PaymentMethod::Known(KnownMethod::Bolt12) => {
                let url = self
                    .mint_url
                    .join_paths(&["v1", "melt", "quote", "bolt12", quote_id])?;

                let auth_token = self
                    .get_auth_token(
                        Method::Get,
                        RoutePath::MeltQuote(PaymentMethod::Known(KnownMethod::Bolt12).to_string()),
                    )
                    .await?;

                let response: cdk_common::nut25::MeltQuoteBolt12Response<String> =
                    self.transport.http_get(url, auth_token).await?;

                Ok(MeltQuoteResponse::Bolt12(response))
            }
            PaymentMethod::Custom(method_name) => {
                let url =
                    self.mint_url
                        .join_paths(&["v1", "melt", "quote", method_name, quote_id])?;

                let auth_token = self
                    .get_auth_token(Method::Get, RoutePath::MeltQuote(method_name.clone()))
                    .await?;

                let response: cdk_common::nut05::MeltQuoteCustomResponse<String> =
                    self.transport.http_get(url, auth_token).await?;

                Ok(MeltQuoteResponse::Custom((method.clone(), response)))
            }
        }
    }

    /// Melt [NUT-05]
    /// [Nut-08] Lightning fee return if outputs defined
    #[instrument(skip(self, request), fields(mint_url = %self.mint_url))]
    async fn post_melt(
        &self,
        method: &PaymentMethod,
        request: MeltRequest<String>,
    ) -> Result<MeltQuoteResponse<String>, Error> {
        let auth_token = self
            .get_auth_token(Method::Post, RoutePath::Melt(method.to_string()))
            .await?;

        let path = match method {
            PaymentMethod::Known(KnownMethod::Bolt11) => {
                nut19::Path::Custom("/v1/melt/bolt11".to_string())
            }
            PaymentMethod::Known(KnownMethod::Bolt12) => {
                nut19::Path::Custom("/v1/melt/bolt12".to_string())
            }
            PaymentMethod::Custom(m) => nut19::Path::custom_melt(m),
        };

        match method {
            PaymentMethod::Known(KnownMethod::Bolt11) => {
                let res: cdk_common::nuts::MeltQuoteBolt11Response<String> = self
                    .retriable_http_request(nut19::Method::Post, path, auth_token, &request)
                    .await?;
                Ok(MeltQuoteResponse::Bolt11(res))
            }
            PaymentMethod::Known(KnownMethod::Bolt12) => {
                let res: cdk_common::nuts::MeltQuoteBolt12Response<String> = self
                    .retriable_http_request(nut19::Method::Post, path, auth_token, &request)
                    .await?;
                Ok(MeltQuoteResponse::Bolt12(res))
            }
            _ => {
                let res: cdk_common::nuts::MeltQuoteCustomResponse<String> = self
                    .retriable_http_request(nut19::Method::Post, path, auth_token, &request)
                    .await?;
                Ok(MeltQuoteResponse::Custom((method.clone(), res)))
            }
        }
    }

    /// Swap Token [NUT-03]
    #[instrument(skip(self, swap_request), fields(mint_url = %self.mint_url))]
    async fn post_swap(&self, swap_request: SwapRequest) -> Result<SwapResponse, Error> {
        let auth_token = self.get_auth_token(Method::Post, RoutePath::Swap).await?;

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

    async fn get_auth_wallet(&self) -> Option<AuthWallet> {
        self.auth_wallet.read().await.clone()
    }

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
        let auth_token = self
            .get_auth_token(Method::Post, RoutePath::Checkstate)
            .await?;

        self.transport.http_post(url, auth_token, &request).await
    }

    /// Restore request [NUT-13]
    #[instrument(skip(self, request), fields(mint_url = %self.mint_url))]
    async fn post_restore(&self, request: RestoreRequest) -> Result<RestoreResponse, Error> {
        let url = self.mint_url.join_paths(&["v1", "restore"])?;
        let auth_token = self
            .get_auth_token(Method::Post, RoutePath::Restore)
            .await?;

        self.transport.http_post(url, auth_token, &request).await
    }
}

/// Http Client

#[derive(Debug, Clone)]
pub struct AuthHttpClient<T>
where
    T: Transport + Send + Sync + 'static,
{
    transport: Arc<T>,
    mint_url: MintUrl,
    cat: Arc<RwLock<AuthToken>>,
}

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

#[cfg(test)]
mod tests {
    use std::fmt;
    use std::str::FromStr;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use serde::de::DeserializeOwned;

    use super::*;
    use crate::nuts::nut04::MintQuoteCustomRequest;

    /// A mock transport that captures the serialized POST payload and returns
    /// a canned JSON response. Follows the same canned-response pattern as
    /// `MockMintConnector` in `wallet/test_utils.rs`.
    #[derive(Clone, Default)]
    struct MockTransport {
        /// The last payload serialized by `http_post`, captured as JSON.
        captured_payload: Arc<Mutex<Option<serde_json::Value>>>,
        /// Canned JSON string returned by `http_post`.
        post_response: Arc<Mutex<Option<String>>>,
    }

    impl fmt::Debug for MockTransport {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("MockTransport").finish()
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl Transport for MockTransport {
        #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
        async fn resolve_dns_txt(&self, _domain: &str) -> Result<Vec<String>, Error> {
            unimplemented!()
        }

        fn with_proxy(
            &mut self,
            _proxy: Url,
            _host_matcher: Option<&str>,
            _accept_invalid_certs: bool,
        ) -> Result<(), Error> {
            Ok(())
        }

        async fn http_get<R>(&self, _url: Url, _auth: Option<AuthToken>) -> Result<R, Error>
        where
            R: DeserializeOwned,
        {
            unimplemented!()
        }

        async fn http_post<P, R>(
            &self,
            _url: Url,
            _auth_token: Option<AuthToken>,
            payload: &P,
        ) -> Result<R, Error>
        where
            P: serde::Serialize + ?Sized + Send + Sync,
            R: DeserializeOwned,
        {
            // Capture the serialized payload for test assertions
            let value = serde_json::to_value(payload).map_err(|e| Error::Custom(e.to_string()))?;
            *self.captured_payload.lock().expect("lock") = Some(value);

            // Return the canned response
            let json = self
                .post_response
                .lock()
                .expect("lock")
                .clone()
                .expect("no mock response set");
            serde_json::from_str(&json).map_err(|e| Error::Custom(e.to_string()))
        }
    }

    /// Regression test: `post_mint_quote` must send only the
    /// `MintQuoteCustomRequest` as the JSON body for custom payment methods,
    /// not the `(PaymentMethod, MintQuoteCustomRequest)` tuple which
    /// serializes as a JSON array.
    #[tokio::test]
    async fn test_post_mint_quote_custom_sends_request_object() {
        // Build a canned MintQuoteCustomResponse<String> for the mock
        let canned_response = MintQuoteCustomResponse::<String> {
            quote: "test-quote-id".to_string(),
            request: "paypal://pay?id=123".to_string(),
            amount: Some(cdk_common::Amount::from(1000)),
            unit: Some(cdk_common::CurrencyUnit::Sat),
            expiry: Some(9999999),
            pubkey: None,
            extra: serde_json::Value::Null,
        };
        let canned_json = serde_json::to_string(&canned_response).expect("serialize response");

        let transport = MockTransport {
            captured_payload: Arc::new(Mutex::new(None)),
            post_response: Arc::new(Mutex::new(Some(canned_json))),
        };
        let captured = transport.captured_payload.clone();

        let mint_url = MintUrl::from_str("https://mint.example.com").expect("parse url");
        let client = HttpClient::with_transport(mint_url, transport, None);

        let request = MintQuoteRequest::Custom {
            method: PaymentMethod::Custom("paypal".to_string()),
            request: MintQuoteCustomRequest {
                amount: cdk_common::Amount::from(1000),
                unit: cdk_common::CurrencyUnit::Sat,
                description: None,
                pubkey: None,
                extra: serde_json::Value::Null,
            },
        };

        let result = client.post_mint_quote(request).await;
        assert!(
            result.is_ok(),
            "post_mint_quote should succeed: {:?}",
            result.err()
        );

        // Verify the payload sent to the transport was a JSON object (not an array)
        let payload = captured
            .lock()
            .expect("lock")
            .clone()
            .expect("payload was captured");
        assert!(
            payload.is_object(),
            "Custom mint quote body sent to transport must be a JSON object, got: {payload}"
        );

        // Verify the payload deserializes as MintQuoteCustomRequest
        let parsed: Result<MintQuoteCustomRequest, _> = serde_json::from_value(payload.clone());
        assert!(
            parsed.is_ok(),
            "Transport payload must deserialize as MintQuoteCustomRequest: {:?}",
            parsed.err()
        );

        // Verify the actual field values round-tripped correctly
        let parsed = parsed.expect("already checked");
        assert_eq!(parsed.amount, cdk_common::Amount::from(1000));
        assert_eq!(parsed.unit, cdk_common::CurrencyUnit::Sat);
    }
}

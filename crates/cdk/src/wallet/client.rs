//! Wallet client

use reqwest::Client;
use serde_json::Value;
use tracing::instrument;
use url::Url;

use super::Error;
use crate::error::ErrorResponse;
use crate::mint_url::MintUrl;
use crate::nuts::nut15::Mpp;
use crate::nuts::nutdlc::{
    DLCRegistrationResponse, DLCStatusResponse, PostDLCPayoutRequest, PostDLCPayoutResponse,
    PostDLCRegistrationRequest, PostSettleDLCRequest, SettleDLCResponse,
};
use crate::nuts::{
    BlindedMessage, CheckStateRequest, CheckStateResponse, CurrencyUnit, Id, KeySet, KeysResponse,
    KeysetResponse, MeltBolt11Request, MeltQuoteBolt11Request, MeltQuoteBolt11Response,
    MintBolt11Request, MintBolt11Response, MintInfo, MintQuoteBolt11Request,
    MintQuoteBolt11Response, PreMintSecrets, Proof, PublicKey, RestoreRequest, RestoreResponse,
    SwapRequest, SwapResponse,
};
use crate::{Amount, Bolt11Invoice};

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

    /// Get Active Mint Keys [NUT-01]
    #[instrument(skip(self), fields(mint_url = %mint_url))]
    pub async fn get_mint_keys(&self, mint_url: MintUrl) -> Result<Vec<KeySet>, Error> {
        let url = mint_url.join_paths(&["v1", "keys"])?;
        let keys = self.inner.get(url).send().await?.json::<Value>().await?;

        match serde_json::from_value::<KeysResponse>(keys.clone()) {
            Ok(keys_response) => Ok(keys_response.keysets),
            Err(_) => Err(ErrorResponse::from_value(keys)?.into()),
        }
    }

    /// Get Keyset Keys [NUT-01]
    #[instrument(skip(self), fields(mint_url = %mint_url))]
    pub async fn get_mint_keyset(&self, mint_url: MintUrl, keyset_id: Id) -> Result<KeySet, Error> {
        let url = mint_url.join_paths(&["v1", "keys", &keyset_id.to_string()])?;
        let keys = self.inner.get(url).send().await?.json::<Value>().await?;

        match serde_json::from_value::<KeysResponse>(keys.clone()) {
            Ok(keys_response) => Ok(keys_response.keysets[0].clone()),
            Err(_) => Err(ErrorResponse::from_value(keys)?.into()),
        }
    }

    /// Get Keysets [NUT-02]
    #[instrument(skip(self), fields(mint_url = %mint_url))]
    pub async fn get_mint_keysets(&self, mint_url: MintUrl) -> Result<KeysetResponse, Error> {
        let url = mint_url.join_paths(&["v1", "keysets"])?;
        let res = self.inner.get(url).send().await?.json::<Value>().await?;

        match serde_json::from_value::<KeysetResponse>(res.clone()) {
            Ok(keyset_response) => Ok(keyset_response),
            Err(_) => Err(ErrorResponse::from_value(res)?.into()),
        }
    }

    /// Mint Quote [NUT-04]
    #[instrument(skip(self), fields(mint_url = %mint_url))]
    pub async fn post_mint_quote(
        &self,
        mint_url: MintUrl,
        amount: Amount,
        unit: CurrencyUnit,
        description: Option<String>,
    ) -> Result<MintQuoteBolt11Response, Error> {
        let url = mint_url.join_paths(&["v1", "mint", "quote", "bolt11"])?;

        let request = MintQuoteBolt11Request {
            amount,
            unit,
            description,
        };

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
    pub async fn get_mint_quote_status(
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
    #[instrument(skip(self, quote, premint_secrets), fields(mint_url = %mint_url))]
    pub async fn post_mint(
        &self,
        mint_url: MintUrl,
        quote: &str,
        premint_secrets: PreMintSecrets,
    ) -> Result<MintBolt11Response, Error> {
        let url = mint_url.join_paths(&["v1", "mint", "bolt11"])?;

        let request = MintBolt11Request {
            quote: quote.to_string(),
            outputs: premint_secrets.blinded_messages(),
        };

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
    pub async fn post_melt_quote(
        &self,
        mint_url: MintUrl,
        unit: CurrencyUnit,
        request: Bolt11Invoice,
        mpp_amount: Option<Amount>,
    ) -> Result<MeltQuoteBolt11Response, Error> {
        let url = mint_url.join_paths(&["v1", "melt", "quote", "bolt11"])?;

        let options = mpp_amount.map(|amount| Mpp { amount });

        let request = MeltQuoteBolt11Request {
            request,
            unit,
            options,
        };

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
    pub async fn get_melt_quote_status(
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
    #[instrument(skip(self, quote, inputs, outputs), fields(mint_url = %mint_url))]
    pub async fn post_melt(
        &self,
        mint_url: MintUrl,
        quote: String,
        inputs: Vec<Proof>,
        outputs: Option<Vec<BlindedMessage>>,
    ) -> Result<MeltQuoteBolt11Response, Error> {
        let url = mint_url.join_paths(&["v1", "melt", "bolt11"])?;

        let request = MeltBolt11Request {
            quote,
            inputs,
            outputs,
        };

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

    /// Split Token [NUT-06]
    #[instrument(skip(self, swap_request), fields(mint_url = %mint_url))]
    pub async fn post_swap(
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
    pub async fn get_mint_info(&self, mint_url: MintUrl) -> Result<MintInfo, Error> {
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
    #[instrument(skip(self), fields(mint_url = %mint_url))]
    pub async fn post_check_state(
        &self,
        mint_url: MintUrl,
        ys: Vec<PublicKey>,
    ) -> Result<CheckStateResponse, Error> {
        let url = mint_url.join_paths(&["v1", "checkstate"])?;
        let request = CheckStateRequest { ys };

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
    pub async fn post_restore(
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

    /// Fund a DLC
    pub async fn post_register_dlc(
        &self,
        mint_url: MintUrl,
        fund_dlc_request: PostDLCRegistrationRequest,
    ) -> Result<DLCRegistrationResponse, Error> {
        let url = mint_url.join_paths(&["v1", "dlc", "fund"])?;
        let res = self
            .inner
            .post(url)
            .json(&fund_dlc_request)
            .send()
            .await?
            .json::<Value>()
            .await?;

        match serde_json::from_value::<DLCRegistrationResponse>(res.clone()) {
            Ok(dlc_response) => Ok(dlc_response),
            Err(_) => Err(ErrorResponse::from_value(res)?.into()),
        }
    }

    /// Settle DLC
    pub async fn post_settle_dlc(
        &self,
        mint_url: MintUrl,
        settle_dlc_request: PostSettleDLCRequest,
    ) -> Result<SettleDLCResponse, Error> {
        let url = mint_url.join_paths(&["v1", "dlc", "settle"])?;
        let res = self
            .inner
            .post(url)
            .json(&settle_dlc_request)
            .send()
            .await?
            .json::<Value>()
            .await?;

        match serde_json::from_value::<SettleDLCResponse>(res.clone()) {
            Ok(settle_dlc_response) => Ok(settle_dlc_response),
            Err(_) => Err(ErrorResponse::from_value(res)?.into()),
        }
    }

    /// Get status of DLC
    pub async fn status(
        &self,
        mint_url: MintUrl,
        dlc_root: &str,
    ) -> Result<DLCStatusResponse, Error> {
        let url = mint_url.join_paths(&["v1", "dlc", "status", dlc_root])?;
        let res = self.inner.get(url).send().await?.json::<Value>().await?;

        match serde_json::from_value::<DLCStatusResponse>(res.clone()) {
            Ok(dlc_status_response) => Ok(dlc_status_response),
            Err(_) => Err(ErrorResponse::from_value(res)?.into()),
        }
    }

    /// Claim payout for DLC
    pub async fn payout(
        &self,
        mint_url: MintUrl,
        request: PostDLCPayoutRequest,
    ) -> Result<PostDLCPayoutResponse, Error> {
        let url = mint_url.join_paths(&["v1", "dlc", "payout"])?;
        let res = self
            .inner
            .post(url)
            .json(&request)
            .send()
            .await?
            .json::<Value>()
            .await?;

        match serde_json::from_value::<PostDLCPayoutResponse>(res.clone()) {
            Ok(dlc_status_response) => Ok(dlc_status_response),
            Err(_) => Err(ErrorResponse::from_value(res)?.into()),
        }
    }
}

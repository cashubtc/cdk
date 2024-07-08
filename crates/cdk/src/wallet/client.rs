//! Wallet client

use reqwest::Client;
use serde_json::Value;
use tracing::instrument;
use url::Url;

use super::Error;
use crate::error::ErrorResponse;
use crate::nuts::nut05::MeltBolt11Response;
use crate::nuts::nut15::Mpp;
use crate::nuts::{
    BlindedMessage, CheckStateRequest, CheckStateResponse, CurrencyUnit, Id, KeySet, KeysResponse,
    KeysetResponse, MeltBolt11Request, MeltQuoteBolt11Request, MeltQuoteBolt11Response,
    MintBolt11Request, MintBolt11Response, MintInfo, MintQuoteBolt11Request,
    MintQuoteBolt11Response, PreMintSecrets, Proof, PublicKey, RestoreRequest, RestoreResponse,
    SwapRequest, SwapResponse,
};
use crate::{Amount, Bolt11Invoice};

fn join_url(url: Url, paths: &[&str]) -> Result<Url, Error> {
    let mut url = url;
    for path in paths {
        if !url.path().ends_with('/') {
            url.path_segments_mut()
                .map_err(|_| Error::UrlPathSegments)?
                .push(path);
        } else {
            url.path_segments_mut()
                .map_err(|_| Error::UrlPathSegments)?
                .pop()
                .push(path);
        }
    }

    Ok(url)
}

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

    /// Get Active Mint Keys [NUT-01]
    #[instrument(skip(self), fields(mint_url = %mint_url))]
    pub async fn get_mint_keys(&self, mint_url: Url) -> Result<Vec<KeySet>, Error> {
        let url = join_url(mint_url, &["v1", "keys"])?;
        let keys = self.inner.get(url).send().await?.json::<Value>().await?;

        match serde_json::from_value::<KeysResponse>(keys.clone()) {
            Ok(keys_response) => Ok(keys_response.keysets),
            Err(_) => Err(ErrorResponse::from_value(keys)?.into()),
        }
    }

    /// Get Keyset Keys [NUT-01]
    #[instrument(skip(self), fields(mint_url = %mint_url))]
    pub async fn get_mint_keyset(&self, mint_url: Url, keyset_id: Id) -> Result<KeySet, Error> {
        let url = join_url(mint_url, &["v1", "keys", &keyset_id.to_string()])?;
        let keys = self.inner.get(url).send().await?.json::<Value>().await?;

        match serde_json::from_value::<KeysResponse>(keys.clone()) {
            Ok(keys_response) => Ok(keys_response.keysets[0].clone()),
            Err(_) => Err(ErrorResponse::from_value(keys)?.into()),
        }
    }

    /// Get Keysets [NUT-02]
    #[instrument(skip(self), fields(mint_url = %mint_url))]
    pub async fn get_mint_keysets(&self, mint_url: Url) -> Result<KeysetResponse, Error> {
        let url = join_url(mint_url, &["v1", "keysets"])?;
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
        mint_url: Url,
        amount: Amount,
        unit: CurrencyUnit,
    ) -> Result<MintQuoteBolt11Response, Error> {
        let url = join_url(mint_url, &["v1", "mint", "quote", "bolt11"])?;

        let request = MintQuoteBolt11Request { amount, unit };

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
        mint_url: Url,
        quote_id: &str,
    ) -> Result<MintQuoteBolt11Response, Error> {
        let url = join_url(mint_url, &["v1", "mint", "quote", "bolt11", quote_id])?;

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
        mint_url: Url,
        quote: &str,
        premint_secrets: PreMintSecrets,
    ) -> Result<MintBolt11Response, Error> {
        let url = join_url(mint_url, &["v1", "mint", "bolt11"])?;

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
    #[instrument(skip(self), fields(mint_url = %mint_url))]
    pub async fn post_melt_quote(
        &self,
        mint_url: Url,
        unit: CurrencyUnit,
        request: Bolt11Invoice,
        mpp_amount: Option<Amount>,
    ) -> Result<MeltQuoteBolt11Response, Error> {
        let url = join_url(mint_url, &["v1", "melt", "quote", "bolt11"])?;

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
        mint_url: Url,
        quote_id: &str,
    ) -> Result<MeltQuoteBolt11Response, Error> {
        let url = join_url(mint_url, &["v1", "melt", "quote", "bolt11", quote_id])?;

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
        mint_url: Url,
        quote: String,
        inputs: Vec<Proof>,
        outputs: Option<Vec<BlindedMessage>>,
    ) -> Result<MeltBolt11Response, Error> {
        let url = join_url(mint_url, &["v1", "melt", "bolt11"])?;

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
            Ok(melt_quote_response) => Ok(melt_quote_response.into()),
            Err(_) => {
                if let Ok(res) = serde_json::from_value::<MeltBolt11Response>(res.clone()) {
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
        mint_url: Url,
        swap_request: SwapRequest,
    ) -> Result<SwapResponse, Error> {
        let url = join_url(mint_url, &["v1", "swap"])?;

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
    pub async fn get_mint_info(&self, mint_url: Url) -> Result<MintInfo, Error> {
        let url = join_url(mint_url, &["v1", "info"])?;

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
        mint_url: Url,
        ys: Vec<PublicKey>,
    ) -> Result<CheckStateResponse, Error> {
        let url = join_url(mint_url, &["v1", "checkstate"])?;
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
        mint_url: Url,
        request: RestoreRequest,
    ) -> Result<RestoreResponse, Error> {
        let url = join_url(mint_url, &["v1", "restore"])?;

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

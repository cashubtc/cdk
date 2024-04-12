use reqwest::Client;
use serde_json::Value;
use url::Url;

use crate::error::{Error, ErrorResponse};
use crate::nuts::{
    BlindedMessage, CheckStateRequest, CheckStateResponse, CurrencyUnit, Id, KeySet, KeysResponse,
    KeysetResponse, MeltBolt11Request, MeltBolt11Response, MeltQuoteBolt11Request,
    MeltQuoteBolt11Response, MintBolt11Request, MintBolt11Response, MintInfo,
    MintQuoteBolt11Request, MintQuoteBolt11Response, PreMintSecrets, Proof, PublicKey,
    RestoreRequest, RestoreResponse, SwapRequest, SwapResponse,
};
use crate::{Amount, Bolt11Invoice};

fn join_url(url: Url, paths: &[&str]) -> Result<Url, Error> {
    let mut url = url;
    for path in paths {
        if !url.path().ends_with('/') {
            url.path_segments_mut()
                .map_err(|_| Error::CustomError("Url Path Segmants".to_string()))?
                .push(path);
        } else {
            url.path_segments_mut()
                .map_err(|_| Error::CustomError("Url Path Segmants".to_string()))?
                .pop()
                .push(path);
        }
    }

    Ok(url)
}

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
    pub fn new() -> Self {
        Self {
            inner: Client::new(),
        }
    }

    /// Get Active Mint Keys [NUT-01]
    pub async fn get_mint_keys(&self, mint_url: Url) -> Result<Vec<KeySet>, Error> {
        let url = join_url(mint_url, &["v1", "keys"])?;
        let keys = self.inner.get(url).send().await?.json::<Value>().await?;

        let keys: KeysResponse = serde_json::from_value(keys)?;
        Ok(keys.keysets)
    }

    /// Get Keyset Keys [NUT-01]
    pub async fn get_mint_keyset(&self, mint_url: Url, keyset_id: Id) -> Result<KeySet, Error> {
        let url = join_url(mint_url, &["v1", "keys", &keyset_id.to_string()])?;
        let keys = self
            .inner
            .get(url)
            .send()
            .await?
            .json::<KeysResponse>()
            .await?;

        // let keys: KeysResponse = serde_json::from_value(keys)?; //
        // serde_json::from_str(&keys.to_string())?;
        Ok(keys.keysets[0].clone())
    }

    /// Get Keysets [NUT-02]
    pub async fn get_mint_keysets(&self, mint_url: Url) -> Result<KeysetResponse, Error> {
        let url = join_url(mint_url, &["v1", "keysets"])?;
        let res = self.inner.get(url).send().await?.json::<Value>().await?;

        let response: Result<KeysetResponse, serde_json::Error> =
            serde_json::from_value(res.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(ErrorResponse::from_json(&res.to_string())?.into()),
        }
    }

    /// Mint Quote [NUT-04]
    pub async fn post_mint_quote(
        &self,
        mint_url: Url,
        amount: Amount,
        unit: CurrencyUnit,
    ) -> Result<MintQuoteBolt11Response, Error> {
        let url = join_url(mint_url, &["v1", "mint", "quote", "bolt11"])?;

        let request = MintQuoteBolt11Request { amount, unit };

        let res = self.inner.post(url).json(&request).send().await?;

        let status = res.status();

        let response: Result<MintQuoteBolt11Response, serde_json::Error> =
            serde_json::from_value(res.json().await?);

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(ErrorResponse::from_json(&status.to_string())?.into()),
        }
    }

    /// Mint Tokens [NUT-04]
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

        let response: Result<MintBolt11Response, serde_json::Error> =
            serde_json::from_value(res.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(ErrorResponse::from_json(&res.to_string())?.into()),
        }
    }

    /// Melt Quote [NUT-05]
    pub async fn post_melt_quote(
        &self,
        mint_url: Url,
        unit: CurrencyUnit,
        request: Bolt11Invoice,
    ) -> Result<MeltQuoteBolt11Response, Error> {
        let url = join_url(mint_url, &["v1", "melt", "quote", "bolt11"])?;

        let request = MeltQuoteBolt11Request { request, unit };

        let value = self.inner.post(url).json(&request).send().await?;

        let value = value.json::<Value>().await?;

        let response: Result<MeltQuoteBolt11Response, serde_json::Error> =
            serde_json::from_value(value.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(ErrorResponse::from_json(&value.to_string())?.into()),
        }
    }

    /// Melt [NUT-05]
    /// [Nut-08] Lightning fee return if outputs defined
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

        let value = self.inner.post(url).json(&request).send().await?;

        let value = value.json::<Value>().await?;
        let response: Result<MeltBolt11Response, serde_json::Error> =
            serde_json::from_value(value.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(ErrorResponse::from_json(&value.to_string())?.into()),
        }
    }

    /// Split Token [NUT-06]
    pub async fn post_swap(
        &self,
        mint_url: Url,
        swap_request: SwapRequest,
    ) -> Result<SwapResponse, Error> {
        let url = join_url(mint_url, &["v1", "swap"])?;

        let res = self.inner.post(url).json(&swap_request).send().await?;

        let value = res.json::<Value>().await?;
        let response: Result<SwapResponse, serde_json::Error> =
            serde_json::from_value(value.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(ErrorResponse::from_json(&value.to_string())?.into()),
        }
    }

    /// Get Mint Info [NUT-06]
    pub async fn get_mint_info(&self, mint_url: Url) -> Result<MintInfo, Error> {
        let url = join_url(mint_url, &["v1", "info"])?;

        let res = self.inner.get(url).send().await?.json::<Value>().await?;

        let response: Result<MintInfo, serde_json::Error> = serde_json::from_value(res.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(ErrorResponse::from_json(&res.to_string())?.into()),
        }
    }

    /// Spendable check [NUT-07]
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

        let response: Result<CheckStateResponse, serde_json::Error> =
            serde_json::from_value(res.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(ErrorResponse::from_json(&res.to_string())?.into()),
        }
    }

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

        let response: Result<RestoreResponse, serde_json::Error> =
            serde_json::from_value(res.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(ErrorResponse::from_json(&res.to_string())?.into()),
        }
    }
}

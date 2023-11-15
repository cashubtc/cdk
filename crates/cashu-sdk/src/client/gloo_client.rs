//! gloo wasm http Client

use async_trait::async_trait;
use cashu::nuts::nut00::wallet::BlindedMessages;
use cashu::nuts::nut00::{BlindedMessage, Proof};
use cashu::nuts::nut01::Keys;
use cashu::nuts::nut03::RequestMintResponse;
use cashu::nuts::nut04::{MintRequest, PostMintResponse};
use cashu::nuts::nut05::{CheckFeesRequest, CheckFeesResponse};
use cashu::nuts::nut06::{SplitRequest, SplitResponse};
#[cfg(feature = "nut07")]
use cashu::nuts::nut07::{CheckSpendableRequest, CheckSpendableResponse};
use cashu::nuts::nut08::{MeltRequest, MeltResponse};
#[cfg(feature = "nut09")]
use cashu::nuts::MintInfo;
use cashu::nuts::*;
use cashu::{Amount, Bolt11Invoice};
use gloo::net::http::Request;
use serde_json::Value;
use url::Url;

use super::join_url;
use crate::client::{Client, Error};

#[derive(Debug, Clone)]
pub struct HttpClient {}

#[async_trait(?Send)]
impl Client for HttpClient {
    /// Get Mint Keys [NUT-01]
    async fn get_mint_keys(&self, mint_url: Url) -> Result<Keys, Error> {
        let url = join_url(mint_url, "keys")?;
        let keys = Request::get(url.as_str())
            .send()
            .await
            .map_err(|err| Error::Gloo(err.to_string()))?
            .json::<Value>()
            .await
            .map_err(|err| Error::Gloo(err.to_string()))?;

        let keys: Keys = serde_json::from_str(&keys.to_string())?;
        Ok(keys)
    }

    /// Get Keysets [NUT-02]
    async fn get_mint_keysets(&self, mint_url: Url) -> Result<nut02::Response, Error> {
        let url = join_url(mint_url, "keysets")?;
        let res = Request::get(url.as_str())
            .send()
            .await
            .map_err(|err| Error::Gloo(err.to_string()))?
            .json::<Value>()
            .await
            .map_err(|err| Error::Gloo(err.to_string()))?;

        let response: Result<nut02::Response, serde_json::Error> =
            serde_json::from_value(res.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(Error::from_json(&res.to_string())?),
        }
    }

    /// Request Mint [NUT-03]
    async fn get_request_mint(
        &self,
        mint_url: Url,
        amount: Amount,
    ) -> Result<RequestMintResponse, Error> {
        let mut url = join_url(mint_url, "mint")?;
        url.query_pairs_mut()
            .append_pair("amount", &amount.to_sat().to_string());

        let res = Request::get(url.as_str())
            .send()
            .await
            .map_err(|err| Error::Gloo(err.to_string()))?
            .json::<Value>()
            .await
            .map_err(|err| Error::Gloo(err.to_string()))?;

        let response: Result<RequestMintResponse, serde_json::Error> =
            serde_json::from_value(res.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(Error::from_json(&res.to_string())?),
        }
    }

    /// Mint Tokens [NUT-04]
    async fn post_mint(
        &self,
        mint_url: Url,
        blinded_messages: BlindedMessages,
        hash: &str,
    ) -> Result<PostMintResponse, Error> {
        let mut url = join_url(mint_url, "mint")?;
        url.query_pairs_mut().append_pair("hash", hash);

        let request = MintRequest {
            outputs: blinded_messages.blinded_messages,
        };

        let res = Request::post(url.as_str())
            .json(&request)
            .map_err(|err| Error::Gloo(err.to_string()))?
            .send()
            .await
            .map_err(|err| Error::Gloo(err.to_string()))?
            .json::<Value>()
            .await
            .map_err(|err| Error::Gloo(err.to_string()))?;

        let response: Result<PostMintResponse, serde_json::Error> =
            serde_json::from_value(res.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(Error::from_json(&res.to_string())?),
        }
    }

    /// Check Max expected fee [NUT-05]
    async fn post_check_fees(
        &self,
        mint_url: Url,
        invoice: Bolt11Invoice,
    ) -> Result<CheckFeesResponse, Error> {
        let url = join_url(mint_url, "checkfees")?;

        let request = CheckFeesRequest { pr: invoice };

        let res = Request::post(url.as_str())
            .json(&request)
            .map_err(|err| Error::Gloo(err.to_string()))?
            .send()
            .await
            .map_err(|err| Error::Gloo(err.to_string()))?
            .json::<Value>()
            .await
            .map_err(|err| Error::Gloo(err.to_string()))?;

        let response: Result<CheckFeesResponse, serde_json::Error> =
            serde_json::from_value(res.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(Error::from_json(&res.to_string())?),
        }
    }

    /// Melt [NUT-05]
    /// [Nut-08] Lightning fee return if outputs defined
    async fn post_melt(
        &self,
        mint_url: Url,
        proofs: Vec<Proof>,
        invoice: Bolt11Invoice,
        outputs: Option<Vec<BlindedMessage>>,
    ) -> Result<MeltResponse, Error> {
        let url = join_url(mint_url, "melt")?;

        let request = MeltRequest {
            proofs,
            pr: invoice,
            outputs,
        };

        let value = Request::post(url.as_str())
            .json(&request)
            .map_err(|err| Error::Gloo(err.to_string()))?
            .send()
            .await
            .map_err(|err| Error::Gloo(err.to_string()))?
            .json::<Value>()
            .await
            .map_err(|err| Error::Gloo(err.to_string()))?;

        let response: Result<MeltResponse, serde_json::Error> =
            serde_json::from_value(value.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(Error::from_json(&value.to_string())?),
        }
    }

    /// Split Token [NUT-06]
    async fn post_split(
        &self,
        mint_url: Url,
        split_request: SplitRequest,
    ) -> Result<SplitResponse, Error> {
        let url = join_url(mint_url, "split")?;

        let res = Request::post(url.as_str())
            .json(&split_request)
            .map_err(|err| Error::Gloo(err.to_string()))?
            .send()
            .await
            .map_err(|err| Error::Gloo(err.to_string()))?
            .json::<Value>()
            .await
            .map_err(|err| Error::Gloo(err.to_string()))?;

        let response: Result<SplitResponse, serde_json::Error> =
            serde_json::from_value(res.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(Error::from_json(&res.to_string())?),
        }
    }

    /// Spendable check [NUT-07]
    #[cfg(feature = "nut07")]
    async fn post_check_spendable(
        &self,
        mint_url: Url,
        proofs: Vec<nut00::mint::Proof>,
    ) -> Result<CheckSpendableResponse, Error> {
        let url = join_url(mint_url, "check")?;
        let request = CheckSpendableRequest {
            proofs: proofs.to_owned(),
        };

        let res = Request::post(url.as_str())
            .json(&request)
            .map_err(|err| Error::Gloo(err.to_string()))?
            .send()
            .await
            .map_err(|err| Error::Gloo(err.to_string()))?
            .json::<Value>()
            .await
            .map_err(|err| Error::Gloo(err.to_string()))?;

        let response: Result<CheckSpendableResponse, serde_json::Error> =
            serde_json::from_value(res.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(Error::from_json(&res.to_string())?),
        }
    }

    /// Get Mint Info [NUT-09]
    #[cfg(feature = "nut09")]
    async fn get_mint_info(&self, mint_url: Url) -> Result<MintInfo, Error> {
        let url = join_url(mint_url, "info")?;
        let res = Request::get(url.as_str())
            .send()
            .await
            .map_err(|err| Error::Gloo(err.to_string()))?
            .json::<Value>()
            .await
            .map_err(|err| Error::Gloo(err.to_string()))?;

        let response: Result<MintInfo, serde_json::Error> = serde_json::from_value(res.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(Error::from_json(&res.to_string())?),
        }
    }
}

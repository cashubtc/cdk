//! Minreq http Client

use std::println;

use async_trait::async_trait;
#[cfg(feature = "nut09")]
use cashu::nuts::MintInfo;
use cashu::nuts::{
    BlindedMessage, CheckFeesRequest, CheckFeesResponse, Keys, MeltRequest, MeltResponse,
    MintRequest, PostMintResponse, PreMintSecrets, Proof, RequestMintResponse, SplitRequest,
    SplitResponse, *,
};
#[cfg(feature = "nut07")]
use cashu::nuts::{CheckSpendableRequest, CheckSpendableResponse};
use cashu::{Amount, Bolt11Invoice};
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
        let keys = minreq::get(url).send()?.json::<Value>()?;

        let keys: Keys = serde_json::from_str(&keys.to_string())?;
        Ok(keys)
    }

    /// Get Keysets [NUT-02]
    async fn get_mint_keysets(&self, mint_url: Url) -> Result<KeysetResponse, Error> {
        let url = join_url(mint_url, "keysets")?;
        let res = minreq::get(url).send()?.json::<Value>()?;

        let response: Result<KeysetResponse, serde_json::Error> =
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

        let res = minreq::get(url).send()?.json::<Value>()?;

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
        premint_secrets: PreMintSecrets,
        hash: &str,
    ) -> Result<PostMintResponse, Error> {
        let mut url = join_url(mint_url, "mint")?;
        url.query_pairs_mut().append_pair("hash", hash);

        let request = MintRequest {
            outputs: premint_secrets.blinded_messages(),
        };

        let res = minreq::post(url)
            .with_json(&request)?
            .send()?
            .json::<Value>()?;

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

        let res = minreq::post(url)
            .with_json(&request)?
            .send()?
            .json::<Value>()?;

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

        let value = minreq::post(url)
            .with_json(&request)?
            .send()?
            .json::<Value>()?;

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

        let res = minreq::post(url).with_json(&split_request)?.send()?;

        println!("{:?}", res);

        let response: Result<SplitResponse, serde_json::Error> =
            serde_json::from_value(res.json::<Value>()?.clone());

        match response {
            Ok(res) if res.promises.is_some() => Ok(res),
            _ => Err(Error::from_json(&res.json::<Value>()?.to_string())?),
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
        let request = CheckSpendableRequest { proofs };

        let res = minreq::post(url)
            .with_json(&request)?
            .send()?
            .json::<Value>()?;

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

        let res = minreq::get(url).send()?.json::<Value>()?;

        let response: Result<MintInfo, serde_json::Error> = serde_json::from_value(res.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(Error::from_json(&res.to_string())?),
        }
    }
}

//! Minreq http Client

use std::println;

use async_trait::async_trait;
use cashu::nuts::{
    BlindedMessage, Keys, MeltBolt11Request, MeltBolt11Response, MintBolt11Request,
    MintBolt11Response, MintInfo, PreMintSecrets, Proof, SwapRequest, SwapResponse, *,
};
#[cfg(feature = "nut07")]
use cashu::nuts::{CheckSpendableRequest, CheckSpendableResponse};
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

    /// Mint Tokens [NUT-04]
    async fn post_mint(
        &self,
        mint_url: Url,
        quote: &str,
        premint_secrets: PreMintSecrets,
    ) -> Result<MintBolt11Response, Error> {
        let url = join_url(mint_url, "mint")?;

        let request = MintBolt11Request {
            quote: quote.to_string(),
            outputs: premint_secrets.blinded_messages(),
        };

        let res = minreq::post(url)
            .with_json(&request)?
            .send()?
            .json::<Value>()?;

        let response: Result<MintBolt11Response, serde_json::Error> =
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
        quote: String,
        inputs: Vec<Proof>,
        outputs: Option<Vec<BlindedMessage>>,
    ) -> Result<MeltBolt11Response, Error> {
        let url = join_url(mint_url, "melt")?;

        let request = MeltBolt11Request {
            quote,
            inputs,
            outputs,
        };

        let value = minreq::post(url)
            .with_json(&request)?
            .send()?
            .json::<Value>()?;

        let response: Result<MeltBolt11Response, serde_json::Error> =
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
        split_request: SwapRequest,
    ) -> Result<SwapResponse, Error> {
        // TODO: Add to endpoint
        let url = join_url(mint_url, "swap")?;

        let res = minreq::post(url).with_json(&split_request)?.send()?;

        println!("{:?}", res);

        let response: Result<SwapResponse, serde_json::Error> =
            serde_json::from_value(res.json::<Value>()?.clone());

        Ok(response?)
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
    async fn get_mint_info(&self, mint_url: Url) -> Result<MintInfo, Error> {
        let url = join_url(mint_url, "v1")?;
        let url = join_url(url, "info")?;

        println!("{}", url);

        let res = minreq::get(url).send()?.json::<Value>()?;

        let response: Result<MintInfo, serde_json::Error> = serde_json::from_value(res.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => {
                println!("{:?}", response);
                Err(Error::from_json(&res.to_string())?)
            }
        }
    }
}

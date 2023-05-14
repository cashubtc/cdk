//! Client to connet to mint

use std::collections::HashMap;

use bitcoin::Amount;
use k256::PublicKey;
use lightning_invoice::Invoice;
use serde_json::Value;
use url::Url;

use crate::{
    error::Error,
    types::{
        BlindedMessage, BlindedMessages, CheckFeesRequest, CheckFeesResponse,
        CheckSpendableRequest, CheckSpendableResponse, MeltRequest, MeltResponse, MintInfo,
        MintKeySets, MintKeys, MintRequest, PostMintResponse, Proof, RequestMintResponse,
        SplitRequest, SplitResponse,
    },
};

#[derive(Debug, Clone)]
pub struct Client {
    pub mint_url: Url,
}

impl Client {
    pub fn new(mint_url: &str) -> Result<Self, Error> {
        // HACK
        let mut mint_url = String::from(mint_url);
        if !mint_url.ends_with('/') {
            mint_url.push('/');
        }
        let mint_url = Url::parse(&mint_url).unwrap();
        Ok(Self { mint_url })
    }

    /// Get Mint Keys [NUT-01]
    pub async fn get_keys(&self) -> Result<MintKeys, Error> {
        let url = self.mint_url.join("keys")?;
        let keys = minreq::get(url.clone()).send()?.json::<Value>()?;

        let keys: HashMap<u64, String> = match serde_json::from_value(keys.clone()) {
            Ok(keys) => keys,
            Err(_err) => {
                return Err(Error::CustomError(format!(
                    "url: {}, {}",
                    url,
                    serde_json::to_string(&keys)?
                )))
            }
        };

        let mint_keys: HashMap<u64, PublicKey> = keys
            .into_iter()
            .filter_map(|(k, v)| {
                let key = hex::decode(v).ok()?;
                let public_key = PublicKey::from_sec1_bytes(&key).ok()?;
                Some((k, public_key))
            })
            .collect();

        Ok(MintKeys(mint_keys))
    }

    /// Get Keysets [NUT-02]
    pub async fn get_keysets(&self) -> Result<MintKeySets, Error> {
        let url = self.mint_url.join("keysets")?;
        Ok(minreq::get(url).send()?.json::<MintKeySets>()?)
    }

    /// Request Mint [NUT-03]
    pub async fn request_mint(&self, amount: Amount) -> Result<RequestMintResponse, Error> {
        let mut url = self.mint_url.join("mint")?;
        url.query_pairs_mut()
            .append_pair("amount", &amount.to_sat().to_string());

        Ok(minreq::get(url).send()?.json::<RequestMintResponse>()?)
    }

    /// Mint Tokens [NUT-04]
    pub async fn mint(
        &self,
        blinded_messages: BlindedMessages,
        hash: &str,
    ) -> Result<PostMintResponse, Error> {
        let mut url = self.mint_url.join("mint")?;
        url.query_pairs_mut().append_pair("hash", hash);

        let request = MintRequest {
            outputs: blinded_messages.blinded_messages,
        };

        let res = minreq::post(url)
            .with_json(&request)?
            .send()?
            .json::<Value>()?;

        let response: Result<PostMintResponse, serde_json::Error> =
            serde_json::from_value(res.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(Error::CustomError(res.to_string())),
        }
    }

    /// Check Max expected fee [NUT-05]
    pub async fn check_fees(&self, invoice: Invoice) -> Result<CheckFeesResponse, Error> {
        let url = self.mint_url.join("checkfees")?;

        let request = CheckFeesRequest { pr: invoice };

        Ok(minreq::post(url)
            .with_json(&request)?
            .send()?
            .json::<CheckFeesResponse>()?)
    }

    /// Melt [NUT-05]
    /// [Nut-08] Lightning fee return if outputs defined
    pub async fn melt(
        &self,
        proofs: Vec<Proof>,
        invoice: Invoice,
        outputs: Option<Vec<BlindedMessage>>,
    ) -> Result<MeltResponse, Error> {
        let url = self.mint_url.join("melt")?;

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
            Err(_) => Err(Error::CustomError(value.to_string())),
        }
    }

    /// Split Token [NUT-06]
    pub async fn split(&self, split_request: SplitRequest) -> Result<SplitResponse, Error> {
        let url = self.mint_url.join("split")?;

        let res = minreq::post(url)
            .with_json(&split_request)?
            .send()?
            .json::<Value>()?;

        // TODO: need to handle response error
        // specifically token already spent
        println!("Split Res: {:?}", res);

        Ok(serde_json::from_value(res).unwrap())
    }

    /// Spendable check [NUT-07]
    pub async fn check_spendable(
        &self,
        proofs: &Vec<Proof>,
    ) -> Result<CheckSpendableResponse, Error> {
        let url = self.mint_url.join("check")?;
        let request = CheckSpendableRequest {
            proofs: proofs.to_owned(),
        };

        Ok(minreq::post(url)
            .with_json(&request)?
            .send()?
            .json::<CheckSpendableResponse>()?)
    }

    /// Get Mint Info [NUT-09]
    pub async fn get_info(&self) -> Result<MintInfo, Error> {
        let url = self.mint_url.join("info")?;
        let res = minreq::get(url).send()?.json::<Value>()?;

        Ok(serde_json::from_value(res)?)
    }
}

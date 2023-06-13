//! Client to connet to mint

use bitcoin::Amount;
use serde_json::Value;
use url::Url;

pub use crate::Invoice;
use crate::{
    error::Error,
    keyset::{Keys, MintKeySets},
    types::{
        BlindedMessage, BlindedMessages, CheckFeesRequest, CheckFeesResponse,
        CheckSpendableRequest, CheckSpendableResponse, MeltRequest, MeltResponse, MintInfo,
        MintRequest, PostMintResponse, Proof, RequestMintResponse, SplitRequest, SplitResponse,
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
        let mint_url = Url::parse(&mint_url)?;
        Ok(Self { mint_url })
    }

    /// Get Mint Keys [NUT-01]
    pub async fn get_keys(&self) -> Result<Keys, Error> {
        let url = self.mint_url.join("keys")?;
        let keys = minreq::get(url).send()?.json::<Value>()?;

        let keys: Keys = serde_json::from_str(&keys.to_string())?;
        /*
                let keys: BTreeMap<u64, String> = match serde_json::from_value(keys.clone()) {
                    Ok(keys) => keys,
                    Err(_err) => {
                        return Err(Error::CustomError(format!(
                            "url: {}, {}",
                            url,
                            serde_json::to_string(&keys)?
                        )))
                    }
                };

                let mint_keys: BTreeMap<u64, PublicKey> = keys
                    .into_iter()
                    .filter_map(|(k, v)| {
                        let key = hex::decode(v).ok()?;
                        let public_key = PublicKey::from_sec1_bytes(&key).ok()?;
                        Some((k, public_key))
                    })
                    .collect();
        */
        Ok(keys)
    }

    /// Get Keysets [NUT-02]
    pub async fn get_keysets(&self) -> Result<MintKeySets, Error> {
        let url = self.mint_url.join("keysets")?;
        let res = minreq::get(url).send()?.json::<Value>()?;

        let response: Result<MintKeySets, serde_json::Error> = serde_json::from_value(res.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(Error::CustomError(res.to_string())),
        }
    }

    /// Request Mint [NUT-03]
    pub async fn request_mint(&self, amount: Amount) -> Result<RequestMintResponse, Error> {
        let mut url = self.mint_url.join("mint")?;
        url.query_pairs_mut()
            .append_pair("amount", &amount.to_sat().to_string());

        let res = minreq::get(url).send()?.json::<Value>()?;

        let response: Result<RequestMintResponse, serde_json::Error> =
            serde_json::from_value(res.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(Error::CustomError(res.to_string())),
        }
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

        let res = minreq::post(url)
            .with_json(&request)?
            .send()?
            .json::<Value>()?;

        let response: Result<CheckFeesResponse, serde_json::Error> =
            serde_json::from_value(res.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(Error::CustomError(res.to_string())),
        }
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

        let response: Result<SplitResponse, serde_json::Error> =
            serde_json::from_value(res.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(Error::CustomError(res.to_string())),
        }
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

        let res = minreq::post(url)
            .with_json(&request)?
            .send()?
            .json::<Value>()?;

        let response: Result<CheckSpendableResponse, serde_json::Error> =
            serde_json::from_value(res.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(Error::CustomError(res.to_string())),
        }
    }

    /// Get Mint Info [NUT-09]
    pub async fn get_info(&self) -> Result<MintInfo, Error> {
        let url = self.mint_url.join("info")?;
        let res = minreq::get(url).send()?.json::<Value>()?;

        let response: Result<MintInfo, serde_json::Error> = serde_json::from_value(res.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(Error::CustomError(res.to_string())),
        }
    }
}

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
    pub fn new(mint_url: Url) -> Self {
        Self { mint_url }
    }

    /// Get Mint Keys [NUT-01]
    pub async fn get_keys(&self) -> Result<MintKeys, Error> {
        let url = self.mint_url.join("keys")?;
        let keys = minreq::get(url).send()?.json::<HashMap<u64, String>>()?;

        Ok(MintKeys(
            keys.into_iter()
                .map(|(k, v)| {
                    (
                        k,
                        PublicKey::from_sec1_bytes(&hex::decode(v).unwrap()).unwrap(),
                    )
                })
                .collect(),
        ))
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
        payment_hash: &str,
    ) -> Result<PostMintResponse, Error> {
        let mut url = self.mint_url.join("mint")?;
        url.query_pairs_mut()
            .append_pair("payment_hash", payment_hash);

        let request = MintRequest {
            outputs: blinded_messages.blinded_messages,
        };

        Ok(minreq::post(url)
            .with_json(&request)?
            .send()?
            .json::<PostMintResponse>()?)
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

        Ok(minreq::post(url)
            .with_json(&request)?
            .send()?
            .json::<MeltResponse>()?)
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
        // println!("{:?}", res);

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
        Ok(minreq::get(url).send()?.json::<MintInfo>()?)
    }
}

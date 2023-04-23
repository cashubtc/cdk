use lightning_invoice::Invoice;
use url::Url;

use crate::{
    error::Error,
    types::{
        BlindedMessage, CheckFeesRequest, CheckFeesResponse, CheckSpendableRequest,
        CheckSpendableResponse, MeltRequest, MeltResposne, MintInfo, MintKeySets, MintKeys,
        MintRequest, PostMintResponse, Proof, RequestMintResponse, SplitRequest, SplitResponse,
    },
};

pub struct CashuMint {
    url: Url,
}

impl CashuMint {
    pub fn new(url: Url) -> Self {
        Self { url }
    }

    /// Get Mint Keys [NUT-01]
    pub async fn get_keys(&self) -> Result<MintKeys, Error> {
        let url = self.url.join("keys")?;
        Ok(minreq::get(url).send()?.json::<MintKeys>()?)
    }

    /// Get Keysets [NUT-02]
    pub async fn get_keysets(&self) -> Result<MintKeySets, Error> {
        let url = self.url.join("keysets")?;
        Ok(minreq::get(url).send()?.json::<MintKeySets>()?)
    }

    /// Request Mint [NUT-03]
    pub async fn request_mint(&self, amount: u64) -> Result<RequestMintResponse, Error> {
        let mut url = self.url.join("mint")?;
        url.query_pairs_mut()
            .append_pair("amount", &amount.to_string());

        Ok(minreq::get(url).send()?.json::<RequestMintResponse>()?)
    }

    /// Mint Tokens [NUT-04]
    pub async fn mint(
        &self,
        blinded_messages: Vec<BlindedMessage>,
        payment_hash: &str,
    ) -> Result<PostMintResponse, Error> {
        let mut url = self.url.join("mint")?;
        url.query_pairs_mut()
            .append_pair("payment_hash", payment_hash);

        let request = MintRequest {
            outputs: blinded_messages,
        };

        Ok(minreq::post(url)
            .with_json(&request)?
            .send()?
            .json::<PostMintResponse>()?)
    }

    /// Check Max expected fee [NUT-05]
    pub async fn check_fees(&self, invoice: Invoice) -> Result<CheckFeesResponse, Error> {
        let url = self.url.join("checkfees")?;

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
    ) -> Result<MeltResposne, Error> {
        let url = self.url.join("melt")?;

        let request = MeltRequest {
            proofs,
            pr: invoice,
            outputs,
        };

        Ok(minreq::post(url)
            .with_json(&request)?
            .send()?
            .json::<MeltResposne>()?)
    }

    /// Split Token [NUT-06]
    pub async fn split(
        &self,
        amount: u64,
        proofs: Vec<Proof>,
        outputs: Vec<BlindedMessage>,
    ) -> Result<SplitResponse, Error> {
        let url = self.url.join("split")?;

        let request = SplitRequest {
            amount,
            proofs,
            outputs,
        };

        Ok(minreq::post(url)
            .with_json(&request)?
            .send()?
            .json::<SplitResponse>()?)
    }

    /// Spendable check [NUT-07]
    pub async fn check_spendable(
        &self,
        proofs: Vec<Proof>,
    ) -> Result<CheckSpendableResponse, Error> {
        let url = self.url.join("check")?;
        let request = CheckSpendableRequest { proofs };

        Ok(minreq::post(url)
            .with_json(&request)?
            .send()?
            .json::<CheckSpendableResponse>()?)
    }

    /// Get Mint Info [NUT-09]
    pub async fn get_info(&self) -> Result<MintInfo, Error> {
        let url = self.url.join("info")?;
        Ok(minreq::get(url).send()?.json::<MintInfo>()?)
    }
}

use std::ops::Deref;
use std::sync::Arc;

use cashu_ffi::{
    BlindedMessage, BlindedMessages, Bolt11Invoice, CheckFeesResponse, CheckSpendableResponse,
    KeySetResponse, MeltResponse, MintInfo, MintProof, PostMintResponse, Proof,
    RequestMintResponse, SplitRequest, SplitResponse,
};
use cashu_sdk::client::blocking::Client as ClientSdk;

use crate::error::Result;
use crate::{Amount, Keys};

pub struct Client {
    inner: ClientSdk,
}

impl Deref for Client {
    type Target = ClientSdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl Client {
    pub fn new(mint_url: String) -> Result<Self> {
        Ok(Self {
            inner: ClientSdk::new(&mint_url)?,
        })
    }

    pub fn get_keys(&self) -> Result<Arc<Keys>> {
        Ok(Arc::new(self.inner.get_keys()?.into()))
    }

    pub fn get_keysets(&self) -> Result<Arc<KeySetResponse>> {
        Ok(Arc::new(self.inner.get_keysets()?.into()))
    }

    pub fn request_mint(&self, amount: Arc<Amount>) -> Result<Arc<RequestMintResponse>> {
        Ok(Arc::new(
            self.inner.request_mint(*amount.as_ref().deref())?.into(),
        ))
    }

    pub fn mint(
        &self,
        blinded_messages: Arc<BlindedMessages>,
        hash: String,
    ) -> Result<Arc<PostMintResponse>> {
        Ok(Arc::new(
            self.inner
                .mint(blinded_messages.as_ref().deref().clone(), &hash)?
                .into(),
        ))
    }

    pub fn check_fees(&self, invoice: Arc<Bolt11Invoice>) -> Result<Arc<CheckFeesResponse>> {
        Ok(Arc::new(
            self.inner
                .check_fees(invoice.as_ref().deref().clone())?
                .into(),
        ))
    }

    pub fn melt(
        &self,
        proofs: Vec<Arc<Proof>>,
        invoice: Arc<Bolt11Invoice>,
        outputs: Option<Vec<Arc<BlindedMessage>>>,
    ) -> Result<Arc<MeltResponse>> {
        Ok(Arc::new(
            self.inner
                .melt(
                    proofs.iter().map(|p| p.as_ref().deref().clone()).collect(),
                    invoice.as_ref().deref().clone(),
                    outputs.map(|bs| bs.iter().map(|b| b.as_ref().deref().clone()).collect()),
                )?
                .into(),
        ))
    }

    pub fn split(&self, split_request: Arc<SplitRequest>) -> Result<Arc<SplitResponse>> {
        Ok(Arc::new(
            self.inner
                .split(split_request.as_ref().deref().clone())?
                .into(),
        ))
    }

    pub fn check_spendable(
        &self,
        proofs: Vec<Arc<MintProof>>,
    ) -> Result<Arc<CheckSpendableResponse>> {
        Ok(Arc::new(
            self.inner
                .check_spendable(&proofs.iter().map(|p| p.as_ref().deref().clone()).collect())?
                .into(),
        ))
    }

    pub fn get_info(&self) -> Result<Arc<MintInfo>> {
        Ok(Arc::new(self.inner.get_info()?.into()))
    }
}

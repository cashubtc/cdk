use cashu::nuts::nut00::wallet::BlindedMessages;
use cashu::nuts::nut00::{self, BlindedMessage, Proof};
use cashu::nuts::nut01::Keys;
use cashu::nuts::nut02;
use cashu::nuts::nut03::RequestMintResponse;
use cashu::nuts::nut04::PostMintResponse;
use cashu::nuts::nut05::CheckFeesResponse;
use cashu::nuts::nut06::{SplitRequest, SplitResponse};
use cashu::nuts::nut07::CheckSpendableResponse;
use cashu::nuts::nut08::MeltResponse;
use cashu::nuts::nut09::MintInfo;
use cashu::{Amount, Bolt11Invoice};

use super::Error;
use crate::RUNTIME;

#[derive(Debug, Clone)]
pub struct Client {
    pub(crate) client: super::Client,
}

impl Client {
    pub fn new(mint_url: &str) -> Result<Self, Error> {
        Ok(Self {
            client: super::Client::new(mint_url)?,
        })
    }

    pub fn get_keys(&self) -> Result<Keys, Error> {
        RUNTIME.block_on(async { self.client.get_keys().await })
    }

    pub fn get_keysets(&self) -> Result<nut02::Response, Error> {
        RUNTIME.block_on(async { self.client.get_keysets().await })
    }

    pub fn request_mint(&self, amount: Amount) -> Result<RequestMintResponse, Error> {
        RUNTIME.block_on(async { self.client.request_mint(amount).await })
    }

    pub fn mint(
        &self,
        blinded_messages: BlindedMessages,
        hash: &str,
    ) -> Result<PostMintResponse, Error> {
        RUNTIME.block_on(async { self.client.mint(blinded_messages, hash).await })
    }

    pub fn check_fees(&self, invoice: Bolt11Invoice) -> Result<CheckFeesResponse, Error> {
        RUNTIME.block_on(async { self.client.check_fees(invoice).await })
    }

    pub fn melt(
        &self,
        proofs: Vec<Proof>,
        invoice: Bolt11Invoice,
        outputs: Option<Vec<BlindedMessage>>,
    ) -> Result<MeltResponse, Error> {
        RUNTIME.block_on(async { self.client.melt(proofs, invoice, outputs).await })
    }

    pub fn split(&self, split_request: SplitRequest) -> Result<SplitResponse, Error> {
        RUNTIME.block_on(async { self.client.split(split_request).await })
    }

    pub fn check_spendable(
        &self,
        proofs: &Vec<nut00::mint::Proof>,
    ) -> Result<CheckSpendableResponse, Error> {
        RUNTIME.block_on(async { self.client.check_spendable(proofs).await })
    }

    pub fn get_info(&self) -> Result<MintInfo, Error> {
        RUNTIME.block_on(async { self.client.get_info().await })
    }
}

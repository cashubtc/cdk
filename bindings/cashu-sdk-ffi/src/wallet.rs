use std::ops::Deref;
use std::sync::Arc;

use cashu_ffi::{
    BlindedMessages, BlindedSignature, Bolt11Invoice, Proof, RequestMintResponse, Token,
};
use cashu_sdk::types::ProofsStatus;
use cashu_sdk::wallet::Wallet as WalletSdk;

use crate::client::Client;
use crate::error::Result;
use crate::types::{Melted, SendProofs};
use crate::{Amount, Keys, MintProof};

pub struct Wallet {
    inner: WalletSdk,
}

impl Wallet {
    pub fn new(client: Arc<Client>, mint_keys: Arc<Keys>) -> Self {
        Self {
            inner: WalletSdk::new(
                client.as_ref().deref().clone(),
                mint_keys.as_ref().deref().clone(),
            ),
        }
    }

    pub fn check_proofs_spent(&self, proofs: Vec<Arc<MintProof>>) -> Result<Arc<ProofsStatus>> {
        Ok(Arc::new(self.inner.check_proofs_spent(
            &proofs.iter().map(|p| p.as_ref().deref().clone()).collect(),
        )?))
    }

    pub fn request_mint(&self, amount: Arc<Amount>) -> Result<Arc<RequestMintResponse>> {
        Ok(Arc::new(
            self.inner.request_mint(*amount.as_ref().deref())?.into(),
        ))
    }

    pub fn mint_token(&self, amount: Arc<Amount>, hash: String) -> Result<Arc<Token>> {
        Ok(Arc::new(
            self.inner
                .mint_token(*amount.as_ref().deref(), &hash)?
                .into(),
        ))
    }

    pub fn mint(&self, amount: Arc<Amount>, hash: String) -> Result<Vec<Arc<Proof>>> {
        Ok(self
            .inner
            .mint(*amount.as_ref().deref(), &hash)?
            .into_iter()
            .map(|p| Arc::new(p.into()))
            .collect())
    }

    pub fn check_fee(&self, invoice: Arc<Bolt11Invoice>) -> Result<Arc<Amount>> {
        Ok(Arc::new(
            self.inner
                .check_fee(invoice.as_ref().deref().clone())?
                .into(),
        ))
    }

    pub fn receive(&self, encoded_token: String) -> Result<Vec<Arc<Proof>>> {
        Ok(self
            .inner
            .receive(&encoded_token)?
            .into_iter()
            .map(|p| Arc::new(p.into()))
            .collect())
    }

    pub fn process_split_response(
        &self,
        blinded_messages: Arc<BlindedMessages>,
        promises: Vec<Arc<BlindedSignature>>,
    ) -> Result<Vec<Arc<Proof>>> {
        Ok(self
            .inner
            .process_split_response(
                blinded_messages.as_ref().deref().clone(),
                promises.iter().map(|p| p.as_ref().deref().into()).collect(),
            )?
            .into_iter()
            .map(|p| Arc::new(p.into()))
            .collect())
    }

    pub fn send(&self, amount: Arc<Amount>, proofs: Vec<Arc<Proof>>) -> Result<Arc<SendProofs>> {
        Ok(Arc::new(
            self.inner
                .send(
                    *amount.as_ref().deref(),
                    proofs.iter().map(|p| p.as_ref().deref().clone()).collect(),
                )?
                .into(),
        ))
    }

    pub fn melt(
        &self,
        invoice: Arc<Bolt11Invoice>,
        proofs: Vec<Arc<Proof>>,
        fee_reserve: Arc<Amount>,
    ) -> Result<Arc<Melted>> {
        Ok(Arc::new(
            self.inner
                .melt(
                    invoice.as_ref().deref().clone(),
                    proofs.iter().map(|p| p.as_ref().deref().clone()).collect(),
                    *fee_reserve.as_ref().deref(),
                )?
                .into(),
        ))
    }

    pub fn proof_to_token(&self, proofs: Vec<Arc<Proof>>, memo: Option<String>) -> Result<String> {
        Ok(self.inner.proofs_to_token(
            proofs.iter().map(|p| p.as_ref().deref().clone()).collect(),
            memo,
        )?)
    }
}

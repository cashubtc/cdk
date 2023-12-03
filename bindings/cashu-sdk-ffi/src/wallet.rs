use std::ops::Deref;
use std::sync::Arc;

use cashu_ffi::{
    BlindedMessages, BlindedSignature, Bolt11Invoice, Proof, RequestMintResponse, Token,
};
use cashu_sdk::client::minreq_client::HttpClient;
use cashu_sdk::types::ProofsStatus;
use cashu_sdk::url::UncheckedUrl;
use cashu_sdk::wallet::Wallet as WalletSdk;
use once_cell::sync::Lazy;
use tokio::runtime::Runtime;

use crate::error::Result;
use crate::types::{Melted, SendProofs};
use crate::{Amount, Keys};

static RUNTIME: Lazy<Runtime> = Lazy::new(|| Runtime::new().expect("Can't start Tokio runtime"));

pub struct Wallet {
    inner: WalletSdk<HttpClient>,
}

impl Wallet {
    pub fn new(mint_url: &str, mint_keys: Arc<Keys>) -> Self {
        let client = HttpClient {};
        Self {
            inner: WalletSdk::new(
                client,
                UncheckedUrl::new(mint_url),
                mint_keys.as_ref().deref().clone(),
            ),
        }
    }

    pub fn check_proofs_spent(&self, proofs: Vec<Arc<Proof>>) -> Result<Arc<ProofsStatus>> {
        let proofs = RUNTIME.block_on(async {
            self.inner
                .check_proofs_spent(proofs.iter().map(|p| p.as_ref().deref().clone()).collect())
                .await
        })?;

        Ok(Arc::new(proofs))
    }

    pub fn request_mint(&self, amount: Arc<Amount>) -> Result<Arc<RequestMintResponse>> {
        let mint_response = RUNTIME
            .block_on(async { self.inner.request_mint(*amount.as_ref().deref()).await })?
            .into();
        Ok(Arc::new(mint_response))
    }

    pub fn mint_token(&self, amount: Arc<Amount>, hash: String) -> Result<Arc<Token>> {
        let token = RUNTIME
            .block_on(async { self.inner.mint_token(*amount.as_ref().deref(), &hash).await })?;

        Ok(Arc::new(token.into()))
    }

    pub fn mint(&self, amount: Arc<Amount>, hash: String) -> Result<Vec<Arc<Proof>>> {
        let proofs =
            RUNTIME.block_on(async { self.inner.mint(*amount.as_ref().deref(), &hash).await })?;

        Ok(proofs.into_iter().map(|p| Arc::new(p.into())).collect())
    }

    pub fn check_fee(&self, invoice: Arc<Bolt11Invoice>) -> Result<Arc<Amount>> {
        let amount = RUNTIME
            .block_on(async { self.inner.check_fee(invoice.as_ref().deref().clone()).await })?;

        Ok(Arc::new(amount.into()))
    }

    pub fn receive(&self, encoded_token: String) -> Result<Vec<Arc<Proof>>> {
        let proofs = RUNTIME.block_on(async { self.inner.receive(&encoded_token).await })?;

        Ok(proofs.into_iter().map(|p| Arc::new(p.into())).collect())
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
                promises.iter().map(|p| p.as_ref().into()).collect(),
            )?
            .into_iter()
            .map(|p| Arc::new(p.into()))
            .collect())
    }

    pub fn send(&self, amount: Arc<Amount>, proofs: Vec<Arc<Proof>>) -> Result<Arc<SendProofs>> {
        let send_proofs = RUNTIME.block_on(async {
            self.inner
                .send(
                    *amount.as_ref().deref(),
                    proofs.iter().map(|p| p.as_ref().deref().clone()).collect(),
                )
                .await
        })?;

        Ok(Arc::new(send_proofs.into()))
    }

    pub fn melt(
        &self,
        invoice: Arc<Bolt11Invoice>,
        proofs: Vec<Arc<Proof>>,
        fee_reserve: Arc<Amount>,
    ) -> Result<Arc<Melted>> {
        let melted = RUNTIME.block_on(async {
            self.inner
                .melt(
                    invoice.as_ref().deref().clone(),
                    proofs.iter().map(|p| p.as_ref().deref().clone()).collect(),
                    *fee_reserve.as_ref().deref(),
                )
                .await
        })?;

        Ok(Arc::new(melted.into()))
    }

    pub fn proof_to_token(&self, proofs: Vec<Arc<Proof>>, memo: Option<String>) -> Result<String> {
        Ok(self.inner.proofs_to_token(
            proofs.iter().map(|p| p.as_ref().deref().clone()).collect(),
            memo,
        )?)
    }
}

use std::ops::Deref;
use std::sync::{Arc, RwLock};

use cashu_ffi::{
    BlindedSignature, CurrencyUnit, MeltQuote, MintQuote, PreMintSecrets, Proof, Token,
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
    inner: RwLock<WalletSdk<HttpClient>>,
}

impl Wallet {
    pub fn new(
        mint_url: String,
        mint_keys: Arc<Keys>,
        mint_quotes: Vec<Arc<MintQuote>>,
        melt_quotes: Vec<Arc<MeltQuote>>,
    ) -> Self {
        let client = HttpClient {};
        Self {
            inner: WalletSdk::new(
                client,
                UncheckedUrl::new(mint_url),
                mint_quotes
                    .into_iter()
                    .map(|q| q.as_ref().deref().clone())
                    .collect(),
                melt_quotes
                    .into_iter()
                    .map(|q| q.as_ref().deref().clone())
                    .collect(),
                mint_keys.as_ref().deref().clone(),
            )
            .into(),
        }
    }

    pub fn check_proofs_spent(&self, proofs: Vec<Arc<Proof>>) -> Result<Arc<ProofsStatus>> {
        let proofs = RUNTIME.block_on(async {
            self.inner
                .read()
                .unwrap()
                .check_proofs_spent(proofs.iter().map(|p| p.as_ref().deref().clone()).collect())
                .await
        })?;

        Ok(Arc::new(proofs))
    }

    pub fn mint_token(
        &self,
        amount: Arc<Amount>,
        unit: Option<CurrencyUnit>,
        memo: Option<String>,
    ) -> Result<Arc<Token>> {
        let token = RUNTIME.block_on(async {
            self.inner
                .write()
                .unwrap()
                .mint_token(*amount.as_ref().deref(), memo, unit.map(|u| u.into()))
                .await
        })?;

        Ok(Arc::new(token.into()))
    }

    pub fn mint(&self, quote: String) -> Result<Vec<Arc<Proof>>> {
        let proofs = RUNTIME.block_on(async { self.inner.write().unwrap().mint(&quote).await })?;

        Ok(proofs.into_iter().map(|p| Arc::new(p.into())).collect())
    }

    pub fn receive(&self, encoded_token: String) -> Result<Vec<Arc<Proof>>> {
        let proofs = RUNTIME
            .block_on(async { self.inner.write().unwrap().receive(&encoded_token).await })?;

        Ok(proofs.into_iter().map(|p| Arc::new(p.into())).collect())
    }

    pub fn process_swap_response(
        &self,
        blinded_messages: Arc<PreMintSecrets>,
        promises: Vec<Arc<BlindedSignature>>,
    ) -> Result<Vec<Arc<Proof>>> {
        Ok(self
            .inner
            .read()
            .unwrap()
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
                .read()
                .unwrap()
                .send(
                    *amount.as_ref().deref(),
                    proofs.iter().map(|p| p.as_ref().deref().clone()).collect(),
                )
                .await
        })?;

        Ok(Arc::new(send_proofs.into()))
    }

    pub fn melt(&self, quote_id: String, proofs: Vec<Arc<Proof>>) -> Result<Arc<Melted>> {
        let melted = RUNTIME.block_on(async {
            self.inner
                .write()
                .unwrap()
                .melt(
                    &quote_id,
                    proofs.iter().map(|p| p.as_ref().deref().clone()).collect(),
                )
                .await
        })?;

        Ok(Arc::new(melted.into()))
    }

    pub fn proofs_to_token(
        &self,
        proofs: Vec<Arc<Proof>>,
        unit: Option<CurrencyUnit>,
        memo: Option<String>,
    ) -> Result<String> {
        Ok(self.inner.read().unwrap().proofs_to_token(
            proofs.iter().map(|p| p.as_ref().deref().clone()).collect(),
            memo,
            unit.map(|u| u.into()),
        )?)
    }
}

use std::{
    collections::HashMap,
    ops::Deref,
    sync::{Arc, RwLock},
};

use cashu_sdk::mint::Mint as MintSdk;

use crate::error::Result;
use cashu_ffi::{
    Amount, CheckSpendableRequest, CheckSpendableResponse, KeySet, KeySetResponse, MeltRequest,
    MeltResponse, MintKeySet, MintRequest, PostMintResponse, Proof, SplitRequest, SplitResponse,
};

pub struct Mint {
    inner: RwLock<MintSdk>,
}

impl Mint {
    pub fn new(
        secret: String,
        derivation_path: String,
        inactive_keysets: HashMap<String, Arc<MintKeySet>>,
        spent_secrets: Vec<String>,
        max_order: u8,
    ) -> Self {
        let spent_secrets = spent_secrets.into_iter().collect();

        let inactive_keysets = inactive_keysets
            .into_iter()
            .map(|(k, v)| (k, v.as_ref().deref().clone()))
            .collect();

        Self {
            inner: MintSdk::new(
                &secret,
                &derivation_path,
                inactive_keysets,
                spent_secrets,
                max_order,
            )
            .into(),
        }
    }

    pub fn active_keyset_pubkeys(&self) -> Arc<KeySet> {
        Arc::new(self.inner.read().unwrap().active_keyset_pubkeys().into())
    }

    pub fn keysets(&self) -> Arc<KeySetResponse> {
        Arc::new(self.inner.read().unwrap().keysets().into())
    }

    pub fn active_keyset(&self) -> Arc<MintKeySet> {
        Arc::new(self.inner.read().unwrap().active_keyset.clone().into())
    }

    pub fn keyset(&self, id: String) -> Option<Arc<KeySet>> {
        self.inner
            .read()
            .unwrap()
            .keyset(&id)
            .map(|k| Arc::new(k.into()))
    }

    pub fn process_mint_request(
        &self,
        mint_request: Arc<MintRequest>,
    ) -> Result<Arc<PostMintResponse>> {
        Ok(Arc::new(
            self.inner
                .write()
                .unwrap()
                .process_mint_request(mint_request.as_ref().deref().clone())?
                .into(),
        ))
    }

    pub fn proccess_split_request(
        &self,
        split_request: Arc<SplitRequest>,
    ) -> Result<Arc<SplitResponse>> {
        Ok(Arc::new(
            self.inner
                .write()
                .unwrap()
                .process_split_request(split_request.as_ref().deref().clone())?
                .into(),
        ))
    }

    pub fn verify_proof(&self, proof: Arc<Proof>) -> Result<String> {
        Ok(self
            .inner
            .read()
            .unwrap()
            .verify_proof(proof.as_ref().deref())?)
    }

    pub fn check_spendable(
        &self,
        check_spendable: Arc<CheckSpendableRequest>,
    ) -> Result<Arc<CheckSpendableResponse>> {
        Ok(Arc::new(
            self.inner
                .read()
                .unwrap()
                .check_spendable(check_spendable.as_ref().deref())?
                .into(),
        ))
    }

    pub fn verify_melt_request(&self, melt_request: Arc<MeltRequest>) -> Result<()> {
        Ok(self
            .inner
            .write()
            .unwrap()
            .verify_melt_request(melt_request.as_ref().deref())?)
    }

    pub fn process_melt_request(
        &self,
        melt_request: Arc<MeltRequest>,
        preimage: String,
        total_spent: Arc<Amount>,
    ) -> Result<Arc<MeltResponse>> {
        Ok(Arc::new(
            self.inner
                .write()
                .unwrap()
                .process_melt_request(
                    melt_request.as_ref().deref(),
                    &preimage,
                    *total_spent.as_ref().deref(),
                )?
                .into(),
        ))
    }
}

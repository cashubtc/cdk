use std::ops::Deref;
use std::str::FromStr;
use std::sync::{Arc, RwLock};

use cashu_ffi::{
    Amount, CheckSpendableRequest, CheckSpendableResponse, Id, KeySet, KeySetResponse,
    KeysResponse, MeltBolt11Request, MeltBolt11Response, MintBolt11Request, MintBolt11Response,
    Secret, SwapRequest, SwapResponse,
};
use cashu_sdk::mint::Mint as MintSdk;
use cashu_sdk::Mnemonic;

use crate::error::Result;
use crate::types::MintKeySetInfo;
use crate::CashuSdkError;

pub struct Mint {
    inner: RwLock<MintSdk>,
}

impl Mint {
    pub fn new(
        secret: String,
        keysets_info: Vec<Arc<MintKeySetInfo>>,
        spent_secrets: Vec<Arc<Secret>>,
        min_fee_reserve: Arc<Amount>,
        percent_fee_reserve: f32,
    ) -> Result<Self> {
        let spent_secrets = spent_secrets
            .into_iter()
            .map(|s| s.as_ref().deref().clone())
            .collect();

        let keysets = keysets_info
            .into_iter()
            .map(|ik| ik.as_ref().deref().clone())
            .collect();

        let menemonic = Mnemonic::from_str(&secret).map_err(|_| CashuSdkError::Generic {
            err: "Invalid Mnemonic".to_string(),
        })?;

        Ok(Self {
            inner: MintSdk::new(
                menemonic,
                keysets,
                spent_secrets,
                // TODO: quotes
                vec![],
                *min_fee_reserve.as_ref().deref(),
                percent_fee_reserve,
            )
            .into(),
        })
    }

    pub fn keyset_pubkeys(&self, keyset_id: Arc<Id>) -> Option<Arc<KeysResponse>> {
        self.inner
            .read()
            .unwrap()
            .keyset_pubkeys(&keyset_id)
            .map(|keyset| Arc::new(keyset.into()))
    }

    pub fn keysets(&self) -> Arc<KeySetResponse> {
        Arc::new(self.inner.read().unwrap().keysets().into())
    }

    pub fn keyset(&self, id: Arc<Id>) -> Option<Arc<KeySet>> {
        self.inner
            .read()
            .unwrap()
            .keyset(&id)
            .map(|k| Arc::new(k.into()))
    }

    pub fn process_mint_request(
        &self,
        mint_request: Arc<MintBolt11Request>,
    ) -> Result<Arc<MintBolt11Response>> {
        Ok(Arc::new(
            self.inner
                .write()
                .unwrap()
                .process_mint_request(mint_request.as_ref().deref().clone())?
                .into(),
        ))
    }

    pub fn process_swap_request(
        &self,
        split_request: Arc<SwapRequest>,
    ) -> Result<Arc<SwapResponse>> {
        Ok(Arc::new(
            self.inner
                .write()
                .unwrap()
                .process_swap_request(split_request.as_ref().deref().clone())?
                .into(),
        ))
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

    pub fn verify_melt_request(&self, melt_request: Arc<MeltBolt11Request>) -> Result<()> {
        Ok(self
            .inner
            .write()
            .unwrap()
            .verify_melt_request(melt_request.as_ref().deref())?)
    }

    pub fn process_melt_request(
        &self,
        melt_request: Arc<MeltBolt11Request>,
        preimage: String,
        total_spent: Arc<Amount>,
    ) -> Result<Arc<MeltBolt11Response>> {
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

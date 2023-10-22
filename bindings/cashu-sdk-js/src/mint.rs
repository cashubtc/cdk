use std::ops::Deref;

use cashu_js::nuts::nut02::{JsId, JsKeySet, JsKeySetsResponse, JsKeysResponse, JsMintKeySet};
use cashu_js::nuts::nut04::{JsMintRequest, JsPostMintResponse};
use cashu_js::nuts::nut06::{JsSplitRequest, JsSplitResponse};
use cashu_js::nuts::nut07::{JsCheckSpendableRequest, JsCheckSpendableResponse};
use cashu_js::nuts::nut08::{JsMeltRequest, JsMeltResponse};
use cashu_js::JsAmount;
use cashu_sdk::mint::Mint;
use cashu_sdk::nuts::nut01;
use cashu_sdk::nuts::nut02::KeySet;
use wasm_bindgen::prelude::*;

use crate::error::{into_err, Result};

#[wasm_bindgen(js_name = Mint)]
pub struct JsMint {
    inner: Mint,
}

impl Deref for JsMint {
    type Target = Mint;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<Mint> for JsMint {
    fn from(inner: Mint) -> JsMint {
        JsMint { inner }
    }
}

#[wasm_bindgen(js_class = Mint)]
impl JsMint {
    #[wasm_bindgen(constructor)]
    pub fn new(
        secret: String,
        derivation_path: String,
        inactive_keyset: JsValue,
        spent_secrets: JsValue,
        max_order: u8,
        min_fee_reserve: JsAmount,
        percent_fee_reserve: f32,
    ) -> Result<JsMint> {
        let inactive_keyset = serde_wasm_bindgen::from_value(inactive_keyset).map_err(into_err)?;
        let spent_secrets = serde_wasm_bindgen::from_value(spent_secrets).map_err(into_err)?;
        Ok(JsMint {
            inner: Mint::new(
                &secret,
                &derivation_path,
                inactive_keyset,
                spent_secrets,
                max_order,
                *min_fee_reserve.deref(),
                percent_fee_reserve,
            ),
        })
    }

    /// Get Active Keyset Pubkeys
    #[wasm_bindgen(getter)]
    pub fn active_keyset_pubkeys(&self) -> Result<JsKeysResponse> {
        let keyset: KeySet = self.inner.active_keyset.clone().into();

        Ok(nut01::Response { keys: keyset.keys }.into())
    }

    /// Get Keysets
    #[wasm_bindgen(js_name = keySets)]
    pub fn keysets(&self) -> JsKeySetsResponse {
        self.inner.keysets().into()
    }

    /// Get Active Keyset
    #[wasm_bindgen(getter)]
    pub fn active_keyset(&self) -> JsMintKeySet {
        self.inner.active_keyset.clone().into()
    }

    /// Keyset
    #[wasm_bindgen(js_name = KeySet)]
    pub fn keyset(&self, id: JsId) -> Option<JsKeySet> {
        self.inner.keyset(id.deref()).map(|ks| ks.into())
    }

    /// Rotate Keyset
    #[wasm_bindgen(js_name = RotateKeyset)]
    pub fn rotate_keyset(&mut self, secret: String, derivation_path: String, max_order: u8) {
        self.inner.rotate_keyset(secret, derivation_path, max_order);
    }

    /// Process Mint Request
    #[wasm_bindgen(js_name = ProcessMintRequest)]
    pub fn process_mint_request(
        &mut self,
        mint_request: JsMintRequest,
    ) -> Result<JsPostMintResponse> {
        Ok(self
            .inner
            .process_mint_request(mint_request.deref().clone())
            .map_err(into_err)?
            .into())
    }

    /// Process Split Request
    #[wasm_bindgen(js_name = ProcessSplitRequest)]
    pub fn process_split_request(
        &mut self,
        split_request: JsSplitRequest,
    ) -> Result<JsSplitResponse> {
        Ok(self
            .inner
            .process_split_request(split_request.deref().clone())
            .map_err(into_err)?
            .into())
    }

    /// Check Spendable
    #[wasm_bindgen(js_name = CheckSpendable)]
    pub fn check_spendable(
        &mut self,
        check_request: JsCheckSpendableRequest,
    ) -> Result<JsCheckSpendableResponse> {
        Ok(self
            .inner
            .check_spendable(&check_request.deref().clone())
            .map_err(into_err)?
            .into())
    }

    /// Check Verify Melt
    #[wasm_bindgen(js_name = VerifyMelt)]
    pub fn verify_melt(&mut self, melt_request: JsMeltRequest) -> Result<()> {
        self.inner
            .verify_melt_request(melt_request.deref())
            .map_err(into_err)
    }

    /// Process Melt Request
    #[wasm_bindgen(js_name = ProcessMeltRequest)]
    pub fn process_melt_request(
        &mut self,
        melt_request: JsMeltRequest,
        preimage: String,
        total_spent: JsAmount,
    ) -> Result<JsMeltResponse> {
        Ok(self
            .inner
            .process_melt_request(melt_request.deref(), &preimage, *total_spent.deref())
            .map_err(into_err)?
            .into())
    }
}

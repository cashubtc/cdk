use std::ops::Deref;
use std::str::FromStr;

#[cfg(feature = "nut07")]
use cashu_js::nuts::{JsCheckSpendableRequest, JsCheckSpendableResponse};
use cashu_js::nuts::{
    JsId, JsKeySet, JsKeySetsResponse, JsKeysResponse, JsMeltBolt11Request, JsMeltBolt11Response,
    JsSwapRequest, JsSwapResponse,
};
use cashu_js::JsAmount;
use cashu_sdk::mint::Mint;
use cashu_sdk::nuts::{KeySet, KeysResponse};
use cashu_sdk::Mnemonic;
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
        keyset_info: JsValue,
        spent_secrets: JsValue,
        quotes: JsValue,
        min_fee_reserve: JsAmount,
        percent_fee_reserve: f32,
    ) -> Result<JsMint> {
        let keyset_info = serde_wasm_bindgen::from_value(keyset_info).map_err(into_err)?;
        let spent_secrets = serde_wasm_bindgen::from_value(spent_secrets).map_err(into_err)?;

        let quotes = serde_wasm_bindgen::from_value(quotes).map_err(into_err)?;
        Ok(JsMint {
            inner: Mint::new(
                Mnemonic::from_str(&secret).unwrap(),
                keyset_info,
                spent_secrets,
                quotes,
                *min_fee_reserve.deref(),
                percent_fee_reserve,
            ),
        })
    }

    /// Get Active Keyset Pubkeys
    #[wasm_bindgen(getter)]
    pub fn keyset_pubkeys(&self, keyset_id: JsId) -> Result<JsKeysResponse> {
        let keyset: KeySet = self
            .inner
            .keyset(&keyset_id)
            .ok_or(JsError::new("Unknown Keyset"))?
            .clone();

        Ok(KeysResponse {
            keysets: vec![keyset],
        }
        .into())
    }

    /// Get Keysets
    #[wasm_bindgen(js_name = keySets)]
    pub fn keysets(&self) -> JsKeySetsResponse {
        self.inner.keysets().into()
    }

    /// Keyset
    #[wasm_bindgen(js_name = KeySet)]
    pub fn keyset(&self, id: JsId) -> Option<JsKeySet> {
        self.inner.keyset(id.deref()).map(|ks| ks.into())
    }

    /// Process Split Request
    #[wasm_bindgen(js_name = ProcessSwapRequest)]
    pub fn process_swap_request(&mut self, swap_request: JsSwapRequest) -> Result<JsSwapResponse> {
        Ok(self
            .inner
            .process_swap_request(swap_request.deref().clone())
            .map_err(into_err)?
            .into())
    }

    /// Check Spendable
    #[cfg(feature = "nut07")]
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
    pub fn verify_melt(&mut self, melt_request: JsMeltBolt11Request) -> Result<()> {
        self.inner
            .verify_melt_request(melt_request.deref())
            .map_err(into_err)
    }

    /// Process Melt Request
    #[wasm_bindgen(js_name = ProcessMeltRequest)]
    pub fn process_melt_request(
        &mut self,
        melt_request: JsMeltBolt11Request,
        preimage: String,
        total_spent: JsAmount,
    ) -> Result<JsMeltBolt11Response> {
        Ok(self
            .inner
            .process_melt_request(melt_request.deref(), &preimage, *total_spent.deref())
            .map_err(into_err)?
            .into())
    }
}

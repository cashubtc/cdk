use std::ops::Deref;

use cashu::nuts::nut00::wallet::PreMintSecrets;
use wasm_bindgen::prelude::*;

use crate::error::{into_err, Result};
use crate::nuts::nut02::JsId;
use crate::types::JsAmount;

#[wasm_bindgen(js_name = BlindedMessages)]
pub struct JsBlindedMessages {
    inner: PreMintSecrets,
}

impl Deref for JsBlindedMessages {
    type Target = PreMintSecrets;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[wasm_bindgen(js_class = BlindedMessages)]
impl JsBlindedMessages {
    #[wasm_bindgen(js_name = random)]
    pub fn random(keyset_id: JsId, amount: JsAmount) -> Result<JsBlindedMessages> {
        Ok(JsBlindedMessages {
            inner: PreMintSecrets::random(*keyset_id.deref(), *amount.deref()).map_err(into_err)?,
        })
    }

    #[wasm_bindgen(js_name = blank)]
    pub fn blank(keyset_id: JsId, fee_reserve: JsAmount) -> Result<JsBlindedMessages> {
        Ok(JsBlindedMessages {
            inner: PreMintSecrets::blank(*keyset_id.deref(), *fee_reserve.deref())
                .map_err(into_err)?,
        })
    }

    /// Blinded Messages
    #[wasm_bindgen(getter)]
    pub fn blinded_messages(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.blinded_messages()).map_err(into_err)
    }

    /// Secrets
    #[wasm_bindgen(getter)]
    pub fn secrets(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.secrets()).map_err(into_err)
    }

    /// rs
    #[wasm_bindgen(getter)]
    pub fn rs(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.rs()).map_err(into_err)
    }

    /// Amounts
    #[wasm_bindgen(getter)]
    pub fn amounts(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.amounts()).map_err(into_err)
    }
}

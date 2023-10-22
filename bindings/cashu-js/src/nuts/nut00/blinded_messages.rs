use std::ops::Deref;

use cashu::nuts::nut00::wallet::BlindedMessages;
use wasm_bindgen::prelude::*;

use crate::error::{into_err, Result};
use crate::types::JsAmount;

#[wasm_bindgen(js_name = BlindedMessages)]
pub struct JsBlindedMessages {
    inner: BlindedMessages,
}

impl Deref for JsBlindedMessages {
    type Target = BlindedMessages;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[wasm_bindgen(js_class = BlindedMessages)]
impl JsBlindedMessages {
    #[wasm_bindgen(js_name = random)]
    pub fn random(amount: JsAmount) -> Result<JsBlindedMessages> {
        Ok(JsBlindedMessages {
            inner: BlindedMessages::random(*amount.deref()).map_err(into_err)?,
        })
    }

    #[wasm_bindgen(js_name = blank)]
    pub fn blank(fee_reserve: JsAmount) -> Result<JsBlindedMessages> {
        Ok(JsBlindedMessages {
            inner: BlindedMessages::blank(*fee_reserve.deref()).map_err(into_err)?,
        })
    }

    /// Blinded Messages
    #[wasm_bindgen(getter)]
    pub fn blinded_messages(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.blinded_messages).map_err(into_err)
    }

    /// Secrets
    #[wasm_bindgen(getter)]
    pub fn secrets(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.secrets).map_err(into_err)
    }

    /// rs
    #[wasm_bindgen(getter)]
    pub fn rs(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.rs).map_err(into_err)
    }

    /// Amounts
    #[wasm_bindgen(getter)]
    pub fn amounts(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.amounts).map_err(into_err)
    }
}

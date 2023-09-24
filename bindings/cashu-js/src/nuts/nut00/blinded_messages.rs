use std::ops::Deref;

use wasm_bindgen::prelude::*;

use cashu::nuts::nut00::wallet::BlindedMessages;

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

    // TODO: Gettters
}

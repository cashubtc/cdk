use std::ops::Deref;

use cashu::nuts::nut00::BlindedSignature;
use wasm_bindgen::prelude::*;

use crate::nuts::nut01::JsPublicKey;
use crate::nuts::nut02::JsId;
use crate::types::JsAmount;

#[wasm_bindgen(js_name = BlindedSignature)]
pub struct JsBlindedSignature {
    inner: BlindedSignature,
}

impl Deref for JsBlindedSignature {
    type Target = BlindedSignature;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[wasm_bindgen(js_class = BlindedSignature)]
impl JsBlindedSignature {
    #[allow(clippy::new_without_default)]
    #[wasm_bindgen(constructor)]
    pub fn new(id: JsId, amount: JsAmount, c: JsPublicKey) -> Self {
        Self {
            inner: BlindedSignature {
                id: *id.deref(),
                amount: *amount.deref(),
                c: c.deref().clone(),
            },
        }
    }
}

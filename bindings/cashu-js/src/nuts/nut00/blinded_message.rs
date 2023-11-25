use std::ops::Deref;

use cashu::nuts::nut00::BlindedMessage;
use wasm_bindgen::prelude::*;

use crate::nuts::nut01::JsPublicKey;
use crate::nuts::nut02::JsId;
use crate::types::amount::JsAmount;

#[wasm_bindgen(js_name = BlindedMessage)]
pub struct JsBlindedMessage {
    inner: BlindedMessage,
}

impl Deref for JsBlindedMessage {
    type Target = BlindedMessage;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<BlindedMessage> for JsBlindedMessage {
    fn from(inner: BlindedMessage) -> JsBlindedMessage {
        JsBlindedMessage { inner }
    }
}

#[wasm_bindgen(js_class = BlindedMessage)]
impl JsBlindedMessage {
    #[allow(clippy::new_without_default)]
    #[wasm_bindgen(constructor)]
    pub fn new(keyset_id: JsId, amount: JsAmount, b: JsPublicKey) -> Self {
        Self {
            inner: BlindedMessage {
                keyset_id: *keyset_id.deref(),
                amount: *amount.deref(),
                b: b.deref().clone(),
            },
        }
    }

    /// Amount
    #[wasm_bindgen(getter)]
    pub fn amount(&self) -> JsAmount {
        self.inner.amount.into()
    }

    /// B
    #[wasm_bindgen(getter)]
    pub fn b(&self) -> JsPublicKey {
        self.inner.b.clone().into()
    }
}

use std::ops::Deref;

use cdk::nuts::BlindSignature;
use wasm_bindgen::prelude::*;

use crate::nuts::nut01::JsPublicKey;
use crate::nuts::nut02::JsId;
use crate::nuts::JsBlindSignatureDleq;
use crate::types::JsAmount;

#[wasm_bindgen(js_name = BlindSignature)]
pub struct JsBlindSignature {
    inner: BlindSignature,
}

impl Deref for JsBlindSignature {
    type Target = BlindSignature;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[wasm_bindgen(js_class = BlindSignature)]
impl JsBlindSignature {
    #[allow(clippy::new_without_default)]
    #[wasm_bindgen(constructor)]
    pub fn new(
        keyset_id: JsId,
        amount: JsAmount,
        c: JsPublicKey,
        dleq: Option<JsBlindSignatureDleq>,
    ) -> Self {
        Self {
            inner: BlindSignature {
                keyset_id: *keyset_id.deref(),
                amount: *amount.deref(),
                c: *c.deref(),
                dleq: dleq.map(|b| b.deref().clone()),
            },
        }
    }
}

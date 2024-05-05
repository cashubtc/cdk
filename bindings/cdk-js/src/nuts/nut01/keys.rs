use std::ops::Deref;

use cdk::nuts::nut01::Keys;
use wasm_bindgen::prelude::*;

use super::JsPublicKey;
use crate::error::{into_err, Result};
use crate::types::JsAmount;

#[wasm_bindgen(js_name = Keys)]
pub struct JsKeys {
    inner: Keys,
}

impl Deref for JsKeys {
    type Target = Keys;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<Keys> for JsKeys {
    fn from(inner: Keys) -> JsKeys {
        JsKeys { inner }
    }
}

#[wasm_bindgen(js_class = Keys)]
impl JsKeys {
    /// From Hex
    #[wasm_bindgen(constructor)]
    pub fn new(keys: JsValue) -> Result<JsKeys> {
        let keys = serde_wasm_bindgen::from_value(keys).map_err(into_err)?;

        Ok(JsKeys {
            inner: Keys::new(keys),
        })
    }

    /// Keys
    #[wasm_bindgen(js_name = keys)]
    pub fn keys(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.keys()).map_err(into_err)
    }

    /// Amount Key
    #[wasm_bindgen(js_name = amountKey)]
    pub fn amount_key(&self, amount: JsAmount) -> Option<JsPublicKey> {
        self.inner.amount_key(*amount.deref()).map(|k| k.into())
    }
}

use std::ops::Deref;

use cdk::Amount;
use wasm_bindgen::prelude::*;

use crate::error::{into_err, Result};

#[wasm_bindgen(js_name = Amount)]
pub struct JsAmount {
    inner: Amount,
}

impl Deref for JsAmount {
    type Target = Amount;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<Amount> for JsAmount {
    fn from(inner: Amount) -> JsAmount {
        JsAmount { inner }
    }
}

impl From<u64> for JsAmount {
    fn from(amount: u64) -> JsAmount {
        JsAmount {
            inner: Amount::from(amount),
        }
    }
}

#[wasm_bindgen(js_class = Amount)]
impl JsAmount {
    #[wasm_bindgen(constructor)]
    pub fn new(sats: u64) -> Self {
        Self {
            inner: Amount::from(sats),
        }
    }

    /// Split amount returns sat vec of sats
    #[wasm_bindgen(js_name = split)]
    pub fn split(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.split()).map_err(into_err)
    }

    #[wasm_bindgen(getter)]
    pub fn value(&self) -> u64 {
        self.inner.into()
    }
}

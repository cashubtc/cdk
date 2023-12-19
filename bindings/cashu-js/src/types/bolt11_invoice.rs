use std::ops::Deref;
use std::str::FromStr;

use cashu::Bolt11Invoice;
use wasm_bindgen::prelude::*;

use crate::error::{into_err, Result};
use crate::types::JsAmount;

#[wasm_bindgen(js_name = Bolt11Invoice)]
pub struct JsBolt11Invoice {
    inner: Bolt11Invoice,
}

impl Deref for JsBolt11Invoice {
    type Target = Bolt11Invoice;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<Bolt11Invoice> for JsBolt11Invoice {
    fn from(inner: Bolt11Invoice) -> JsBolt11Invoice {
        JsBolt11Invoice { inner }
    }
}

#[wasm_bindgen(js_class = Bolt11Invoice)]
impl JsBolt11Invoice {
    #[wasm_bindgen(constructor)]
    pub fn new(invoice: String) -> Result<JsBolt11Invoice> {
        Ok(JsBolt11Invoice {
            inner: Bolt11Invoice::from_str(&invoice).map_err(into_err)?,
        })
    }

    /// Amount
    #[wasm_bindgen(getter)]
    pub fn amount(&self) -> Option<JsAmount> {
        self.inner
            .amount_milli_satoshis()
            .map(|a| JsAmount::from(a / 1000))
    }

    /// Invoice as string
    #[wasm_bindgen(js_name = asString)]
    pub fn as_string(&self) -> String {
        self.inner.to_string()
    }
}

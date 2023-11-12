use std::ops::Deref;

use cashu_sdk::types::Melted;
use wasm_bindgen::prelude::*;

use crate::error::{into_err, Result};

#[wasm_bindgen(js_name = Melted)]
pub struct JsMelted {
    inner: Melted,
}

impl Deref for JsMelted {
    type Target = Melted;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<Melted> for JsMelted {
    fn from(inner: Melted) -> JsMelted {
        JsMelted { inner }
    }
}

#[wasm_bindgen(js_class = Melted)]
impl JsMelted {
    #[wasm_bindgen(constructor)]
    pub fn new(paid: bool, preimage: Option<String>, change: JsValue) -> Result<JsMelted> {
        let change = serde_wasm_bindgen::from_value(change).map_err(into_err)?;
        Ok(JsMelted {
            inner: Melted {
                paid,
                preimage,
                change,
            },
        })
    }

    /// Get Preimage
    #[wasm_bindgen(getter)]
    pub fn preimage(&self) -> Option<String> {
        self.inner.preimage.clone()
    }

    /// Get Paid
    #[wasm_bindgen(getter)]
    pub fn paid(&self) -> bool {
        self.inner.paid
    }

    /// Get Change
    #[wasm_bindgen(getter)]
    pub fn change(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.change).map_err(into_err)
    }
}

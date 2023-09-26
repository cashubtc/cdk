use std::ops::Deref;

use cashu_sdk::types::ProofsStatus;
use wasm_bindgen::prelude::*;

use crate::error::{into_err, Result};

#[wasm_bindgen(js_name = ProofStatus)]
pub struct JsProofsStatus {
    inner: ProofsStatus,
}

impl Deref for JsProofsStatus {
    type Target = ProofsStatus;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<ProofsStatus> for JsProofsStatus {
    fn from(inner: ProofsStatus) -> JsProofsStatus {
        JsProofsStatus { inner }
    }
}

#[wasm_bindgen(js_class = ProofsStatus)]
impl JsProofsStatus {
    #[wasm_bindgen(constructor)]
    pub fn new(spendable: JsValue, spent: JsValue) -> Result<JsProofsStatus> {
        let spendable = serde_wasm_bindgen::from_value(spendable).map_err(into_err)?;
        let spent = serde_wasm_bindgen::from_value(spent).map_err(into_err)?;
        Ok(JsProofsStatus {
            inner: ProofsStatus { spendable, spent },
        })
    }

    /// Get Spendable
    #[wasm_bindgen(getter)]
    pub fn spendable(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.spendable).map_err(into_err)
    }

    /// Get Spent
    #[wasm_bindgen(getter)]
    pub fn spent(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.spent).map_err(into_err)
    }
}

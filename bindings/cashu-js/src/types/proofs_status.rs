use std::ops::Deref;

use cashu::types::ProofsStatus;
use wasm_bindgen::prelude::*;

use crate::error::{into_err, Result};

#[wasm_bindgen(js_name = ProofsStatus)]
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
    pub fn new(spent_proofs: JsValue, spendable_proofs: JsValue) -> Result<JsProofsStatus> {
        let spent = serde_wasm_bindgen::from_value(spent_proofs).map_err(into_err)?;
        let spendable = serde_wasm_bindgen::from_value(spendable_proofs).map_err(into_err)?;
        Ok(JsProofsStatus {
            inner: ProofsStatus { spent, spendable },
        })
    }

    /// Get Spendable Proofs
    #[wasm_bindgen(getter)]
    pub fn spendable(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.spendable).map_err(into_err)
    }

    /// Get Spent Proofs
    #[wasm_bindgen(getter)]
    pub fn spent_proofs(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.spent).map_err(into_err)
    }
}

use std::ops::Deref;

use cashu::nuts::nut07::{CheckSpendableRequest, CheckSpendableResponse};
use wasm_bindgen::prelude::*;

use crate::error::{into_err, Result};

#[wasm_bindgen(js_name = CheckSpendableRequest)]
pub struct JsCheckSpendableRequest {
    inner: CheckSpendableRequest,
}

impl Deref for JsCheckSpendableRequest {
    type Target = CheckSpendableRequest;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<CheckSpendableRequest> for JsCheckSpendableRequest {
    fn from(inner: CheckSpendableRequest) -> JsCheckSpendableRequest {
        JsCheckSpendableRequest { inner }
    }
}

#[wasm_bindgen(js_class = CheckSpendable)]
impl JsCheckSpendableRequest {
    #[wasm_bindgen(constructor)]
    pub fn new(proofs: JsValue) -> Result<JsCheckSpendableRequest> {
        let proofs = serde_wasm_bindgen::from_value(proofs).map_err(into_err)?;

        Ok(JsCheckSpendableRequest {
            inner: CheckSpendableRequest { proofs },
        })
    }

    /// Get Proofs
    #[wasm_bindgen(getter)]
    pub fn proofs(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.proofs).map_err(into_err)
    }
}

#[wasm_bindgen(js_name = CheckSpendableResponse)]
pub struct JsCheckSpendableResponse {
    inner: CheckSpendableResponse,
}

impl Deref for JsCheckSpendableResponse {
    type Target = CheckSpendableResponse;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<CheckSpendableResponse> for JsCheckSpendableResponse {
    fn from(inner: CheckSpendableResponse) -> JsCheckSpendableResponse {
        JsCheckSpendableResponse { inner }
    }
}

#[wasm_bindgen(js_class = CheckSpendableResponse)]
impl JsCheckSpendableResponse {
    #[wasm_bindgen(constructor)]
    pub fn new(spendable: JsValue, pending: JsValue) -> Result<JsCheckSpendableResponse> {
        let spendable = serde_wasm_bindgen::from_value(spendable).map_err(into_err)?;

        let pending = serde_wasm_bindgen::from_value(pending).map_err(into_err)?;

        Ok(JsCheckSpendableResponse {
            inner: CheckSpendableResponse { spendable, pending },
        })
    }

    /// Get Pending
    #[wasm_bindgen(getter)]
    pub fn pending(&self) -> Box<[JsValue]> {
        self.inner
            .pending
            .iter()
            .map(|p| JsValue::from_bool(*p))
            .collect()
    }

    /// Get Spendable
    #[wasm_bindgen(getter)]
    pub fn spendable(&self) -> Box<[JsValue]> {
        self.inner
            .spendable
            .iter()
            .map(|s| JsValue::from_bool(*s))
            .collect()
    }
}

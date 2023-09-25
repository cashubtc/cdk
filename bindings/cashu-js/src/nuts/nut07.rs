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
    // REVIEW: Use into serde
    #[wasm_bindgen(constructor)]
    pub fn new(proofs: String) -> Result<JsCheckSpendableRequest> {
        let proofs = serde_json::from_str(&proofs).map_err(into_err)?;

        Ok(JsCheckSpendableRequest {
            inner: CheckSpendableRequest { proofs },
        })
    }

    /// Get Proofs
    #[wasm_bindgen(getter)]
    // REVIEW: INTO Serde
    pub fn proofs(&self) -> Result<String> {
        serde_json::to_string(&self.inner.proofs).map_err(into_err)
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
    pub fn new(
        js_spendable: Box<[JsValue]>,
        js_pending: Box<[JsValue]>,
    ) -> Result<JsCheckSpendableResponse> {
        let spendable: Vec<bool> = js_spendable.iter().flat_map(|s| s.as_bool()).collect();

        if spendable.len().ne(&js_spendable.len()) {
            return Err(JsValue::from_str("Wrong value"));
        }

        let pending: Vec<bool> = js_pending.iter().flat_map(|p| p.as_bool()).collect();

        if pending.len().ne(&js_pending.len()) {
            return Err(JsValue::from_str("Wrong value"));
        }

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

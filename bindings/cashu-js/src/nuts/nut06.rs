use std::ops::Deref;

use cashu::nuts::nut06::{SplitRequest, SplitResponse};
use wasm_bindgen::prelude::*;

use crate::error::{into_err, Result};
use crate::types::JsAmount;

#[wasm_bindgen(js_name = SplitRequest)]
pub struct JsSplitRequest {
    inner: SplitRequest,
}

impl Deref for JsSplitRequest {
    type Target = SplitRequest;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<SplitRequest> for JsSplitRequest {
    fn from(inner: SplitRequest) -> JsSplitRequest {
        JsSplitRequest { inner }
    }
}

#[wasm_bindgen(js_class = SplitRequest)]
impl JsSplitRequest {
    #[wasm_bindgen(constructor)]
    pub fn new(proofs: JsValue, outputs: JsValue) -> Result<JsSplitRequest> {
        let proofs = serde_wasm_bindgen::from_value(proofs).map_err(into_err)?;
        let outputs = serde_wasm_bindgen::from_value(outputs).map_err(into_err)?;

        Ok(JsSplitRequest {
            inner: SplitRequest {
                amount: None,
                proofs,
                outputs,
            },
        })
    }

    /// Get Proofs
    #[wasm_bindgen(getter)]
    pub fn proofs(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.proofs).map_err(into_err)
    }

    /// Get Outputs
    #[wasm_bindgen(getter)]
    pub fn outputs(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.outputs).map_err(into_err)
    }

    /// Proofs Amount
    #[wasm_bindgen(js_name = proofsAmount)]
    pub fn proofs_amount(&self) -> JsAmount {
        self.inner.proofs_amount().into()
    }

    /// Output Amount
    #[wasm_bindgen(js_name = outputAmount)]
    pub fn output_amount(&self) -> JsAmount {
        self.inner.output_amount().into()
    }
}

#[wasm_bindgen(js_name = SplitResponse)]
pub struct JsSplitResponse {
    inner: SplitResponse,
}

impl Deref for JsSplitResponse {
    type Target = SplitResponse;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<SplitResponse> for JsSplitResponse {
    fn from(inner: SplitResponse) -> JsSplitResponse {
        JsSplitResponse { inner }
    }
}

#[wasm_bindgen(js_class = SplitResponse)]
impl JsSplitResponse {
    #[wasm_bindgen(constructor)]
    pub fn new(promises: JsValue) -> Result<JsSplitResponse> {
        let promises = serde_wasm_bindgen::from_value(promises).map_err(into_err)?;

        Ok(JsSplitResponse {
            inner: SplitResponse {
                fst: None,
                snd: None,
                promises: Some(promises),
            },
        })
    }

    /// Get Promises
    #[wasm_bindgen(getter)]
    pub fn promises(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.promises).map_err(into_err)
    }

    /// Promises Amount
    #[wasm_bindgen(js_name = promisesAmount)]
    pub fn promises_amount(&self) -> Option<JsAmount> {
        self.inner.promises_amount().map(|a| a.into())
    }
}

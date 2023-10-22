use std::ops::Deref;

use cashu::nuts::nut00::{BlindedMessage, BlindedSignature, Proof};
use cashu::nuts::nut08::{MeltRequest, MeltResponse};
use wasm_bindgen::prelude::*;

use crate::error::{into_err, Result};
use crate::types::{JsAmount, JsBolt11Invoice};

#[wasm_bindgen(js_name = MeltRequest)]
pub struct JsMeltRequest {
    inner: MeltRequest,
}

impl Deref for JsMeltRequest {
    type Target = MeltRequest;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<MeltRequest> for JsMeltRequest {
    fn from(inner: MeltRequest) -> JsMeltRequest {
        JsMeltRequest { inner }
    }
}

#[wasm_bindgen(js_class = MeltRequest)]
impl JsMeltRequest {
    #[wasm_bindgen(constructor)]
    pub fn new(
        proofs: JsValue,
        invoice: JsBolt11Invoice,
        outputs: JsValue,
    ) -> Result<JsMeltRequest> {
        let proofs: Vec<Proof> = serde_wasm_bindgen::from_value(proofs).map_err(into_err)?;
        let outputs: Option<Vec<BlindedMessage>> = if !outputs.is_null() {
            Some(serde_wasm_bindgen::from_value(outputs).map_err(into_err)?)
        } else {
            None
        };

        Ok(JsMeltRequest {
            inner: MeltRequest {
                proofs,
                pr: invoice.deref().clone(),
                outputs,
            },
        })
    }

    /// Get Proofs
    #[wasm_bindgen(getter)]
    pub fn proofs(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.proofs).map_err(into_err)
    }

    /// Get Invoice
    #[wasm_bindgen(getter)]
    pub fn invoice(&self) -> JsBolt11Invoice {
        self.inner.pr.clone().into()
    }

    /// Get outputs
    #[wasm_bindgen(getter)]
    pub fn outputs(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.outputs).map_err(into_err)
    }
}

#[wasm_bindgen(js_name = MeltResponse)]
pub struct JsMeltResponse {
    inner: MeltResponse,
}

impl Deref for JsMeltResponse {
    type Target = MeltResponse;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<MeltResponse> for JsMeltResponse {
    fn from(inner: MeltResponse) -> JsMeltResponse {
        JsMeltResponse { inner }
    }
}

#[wasm_bindgen(js_class = MeltResponse)]
impl JsMeltResponse {
    #[wasm_bindgen(constructor)]
    pub fn new(paid: bool, preimage: Option<String>, change: JsValue) -> Result<JsMeltResponse> {
        let change: Option<Vec<BlindedSignature>> = if change.is_null() {
            Some(serde_wasm_bindgen::from_value(change).map_err(into_err)?)
        } else {
            None
        };

        Ok(JsMeltResponse {
            inner: MeltResponse {
                paid,
                preimage,
                change,
            },
        })
    }

    /// Get Paid
    #[wasm_bindgen(getter)]
    pub fn paid(&self) -> bool {
        self.inner.paid
    }

    /// Get Preimage
    #[wasm_bindgen(getter)]
    pub fn preimage(&self) -> Option<String> {
        self.inner.preimage.clone()
    }

    /// Get Change
    #[wasm_bindgen(getter)]
    pub fn change(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.change).map_err(into_err)
    }

    /// Change Amount
    #[wasm_bindgen(js_name = "changeAmount")]
    pub fn change_amount(&self) -> JsAmount {
        self.inner.change_amount().into()
    }
}

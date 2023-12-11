use std::ops::Deref;

use cashu::nuts::nut00::{BlindedMessage, BlindedSignature, Proof};
use cashu::nuts::nut08::{MeltBolt11Request, MeltBolt11Response};
use wasm_bindgen::prelude::*;

use crate::error::{into_err, Result};
use crate::types::JsAmount;

#[wasm_bindgen(js_name = MeltRequest)]
pub struct JsMeltRequest {
    inner: MeltBolt11Request,
}

impl Deref for JsMeltRequest {
    type Target = MeltBolt11Request;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<MeltBolt11Request> for JsMeltRequest {
    fn from(inner: MeltBolt11Request) -> JsMeltRequest {
        JsMeltRequest { inner }
    }
}

#[wasm_bindgen(js_class = MeltRequest)]
impl JsMeltRequest {
    #[wasm_bindgen(constructor)]
    pub fn new(quote: String, inputs: JsValue, outputs: JsValue) -> Result<JsMeltRequest> {
        let inputs: Vec<Proof> = serde_wasm_bindgen::from_value(inputs).map_err(into_err)?;
        let outputs: Option<Vec<BlindedMessage>> = if !outputs.is_null() {
            Some(serde_wasm_bindgen::from_value(outputs).map_err(into_err)?)
        } else {
            None
        };

        Ok(JsMeltRequest {
            inner: MeltBolt11Request {
                quote,
                inputs,
                outputs,
            },
        })
    }

    /// Get Proofs
    #[wasm_bindgen(getter)]
    pub fn inputs(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.inputs).map_err(into_err)
    }

    /// Get Invoice
    #[wasm_bindgen(getter)]
    pub fn quote(&self) -> String {
        self.inner.quote.clone()
    }

    /// Get outputs
    #[wasm_bindgen(getter)]
    pub fn outputs(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.outputs).map_err(into_err)
    }
}

#[wasm_bindgen(js_name = MeltResponse)]
pub struct JsMeltResponse {
    inner: MeltBolt11Response,
}

impl Deref for JsMeltResponse {
    type Target = MeltBolt11Response;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<MeltBolt11Response> for JsMeltResponse {
    fn from(inner: MeltBolt11Response) -> JsMeltResponse {
        JsMeltResponse { inner }
    }
}

#[wasm_bindgen(js_class = MeltResponse)]
impl JsMeltResponse {
    #[wasm_bindgen(constructor)]
    pub fn new(paid: bool, proof: String, change: JsValue) -> Result<JsMeltResponse> {
        let change: Option<Vec<BlindedSignature>> = if change.is_null() {
            Some(serde_wasm_bindgen::from_value(change).map_err(into_err)?)
        } else {
            None
        };

        Ok(JsMeltResponse {
            inner: MeltBolt11Response {
                proof,
                paid,
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
    pub fn proof(&self) -> String {
        self.inner.proof.clone()
    }

    /// Get Change
    #[wasm_bindgen(getter)]
    pub fn change(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.change).map_err(into_err)
    }

    /// Change Amount
    #[wasm_bindgen(js_name = "changeAmount")]
    pub fn change_amount(&self) -> Option<JsAmount> {
        self.inner.change_amount().map(|a| a.into())
    }
}

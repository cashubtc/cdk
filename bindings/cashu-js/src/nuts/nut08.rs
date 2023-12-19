use std::ops::Deref;

use cashu::nuts::nut00::{BlindedMessage, BlindedSignature, Proof};
use cashu::nuts::nut08::{MeltBolt11Request, MeltBolt11Response};
use wasm_bindgen::prelude::*;

use crate::error::{into_err, Result};
use crate::types::JsAmount;

#[wasm_bindgen(js_name = MeltRequest)]
pub struct JsMeltBolt11Request {
    inner: MeltBolt11Request,
}

impl Deref for JsMeltBolt11Request {
    type Target = MeltBolt11Request;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<MeltBolt11Request> for JsMeltBolt11Request {
    fn from(inner: MeltBolt11Request) -> JsMeltBolt11Request {
        JsMeltBolt11Request { inner }
    }
}

#[wasm_bindgen(js_class = MeltBolt11Request)]
impl JsMeltBolt11Request {
    #[wasm_bindgen(constructor)]
    pub fn new(quote: String, inputs: JsValue, outputs: JsValue) -> Result<JsMeltBolt11Request> {
        let inputs: Vec<Proof> = serde_wasm_bindgen::from_value(inputs).map_err(into_err)?;
        let outputs: Option<Vec<BlindedMessage>> = if !outputs.is_null() {
            Some(serde_wasm_bindgen::from_value(outputs).map_err(into_err)?)
        } else {
            None
        };

        Ok(JsMeltBolt11Request {
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
pub struct JsMeltBolt11Response {
    inner: MeltBolt11Response,
}

impl Deref for JsMeltBolt11Response {
    type Target = MeltBolt11Response;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<MeltBolt11Response> for JsMeltBolt11Response {
    fn from(inner: MeltBolt11Response) -> JsMeltBolt11Response {
        JsMeltBolt11Response { inner }
    }
}

#[wasm_bindgen(js_class = MeltBolt11Response)]
impl JsMeltBolt11Response {
    #[wasm_bindgen(constructor)]
    pub fn new(
        paid: bool,
        payment_preimage: Option<String>,
        change: JsValue,
    ) -> Result<JsMeltBolt11Response> {
        let change: Option<Vec<BlindedSignature>> = if change.is_null() {
            Some(serde_wasm_bindgen::from_value(change).map_err(into_err)?)
        } else {
            None
        };

        Ok(JsMeltBolt11Response {
            inner: MeltBolt11Response {
                payment_preimage,
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
    pub fn payment_preimage(&self) -> Option<String> {
        self.inner.payment_preimage.clone()
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

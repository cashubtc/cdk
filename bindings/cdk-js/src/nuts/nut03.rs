use std::ops::Deref;

use cdk::nuts::{SwapRequest, SwapResponse};
use wasm_bindgen::prelude::*;

use crate::error::{into_err, Result};
use crate::types::JsAmount;

#[wasm_bindgen(js_name = SwapRequest)]
pub struct JsSwapRequest {
    inner: SwapRequest,
}

impl Deref for JsSwapRequest {
    type Target = SwapRequest;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<SwapRequest> for JsSwapRequest {
    fn from(inner: SwapRequest) -> JsSwapRequest {
        JsSwapRequest { inner }
    }
}

#[wasm_bindgen(js_class = SwapRequest)]
impl JsSwapRequest {
    #[wasm_bindgen(constructor)]
    pub fn new(inputs: JsValue, outputs: JsValue) -> Result<JsSwapRequest> {
        let inputs = serde_wasm_bindgen::from_value(inputs).map_err(into_err)?;
        let outputs = serde_wasm_bindgen::from_value(outputs).map_err(into_err)?;

        Ok(JsSwapRequest {
            inner: SwapRequest { inputs, outputs },
        })
    }

    /// Get Proofs
    #[wasm_bindgen(getter)]
    pub fn proofs(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.inputs).map_err(into_err)
    }

    /// Get Outputs
    #[wasm_bindgen(getter)]
    pub fn outputs(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.outputs).map_err(into_err)
    }

    /// Proofs Amount
    #[wasm_bindgen(js_name = proofsAmount)]
    pub fn proofs_amount(&self) -> JsAmount {
        self.inner.input_amount().expect("Amount overflow").into()
    }

    /// Output Amount
    #[wasm_bindgen(js_name = outputAmount)]
    pub fn output_amount(&self) -> JsAmount {
        self.inner.output_amount().expect("Amount overflow").into()
    }
}

#[wasm_bindgen(js_name = SplitResponse)]
pub struct JsSwapResponse {
    inner: SwapResponse,
}

impl Deref for JsSwapResponse {
    type Target = SwapResponse;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<SwapResponse> for JsSwapResponse {
    fn from(inner: SwapResponse) -> JsSwapResponse {
        JsSwapResponse { inner }
    }
}

#[wasm_bindgen(js_class = SplitResponse)]
impl JsSwapResponse {
    #[wasm_bindgen(constructor)]
    pub fn new(signatures: JsValue) -> Result<JsSwapResponse> {
        let signatures = serde_wasm_bindgen::from_value(signatures).map_err(into_err)?;

        Ok(JsSwapResponse {
            inner: SwapResponse { signatures },
        })
    }

    /// Get Promises
    #[wasm_bindgen(getter)]
    pub fn signatures(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.signatures).map_err(into_err)
    }

    /// Promises Amount
    #[wasm_bindgen(js_name = promisesAmount)]
    pub fn promises_amount(&self) -> JsAmount {
        self.inner
            .promises_amount()
            .expect("Amount overflow")
            .into()
    }
}

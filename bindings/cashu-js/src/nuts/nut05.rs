use std::ops::Deref;

use cashu::nuts::nut05::{CheckFeesRequest, CheckFeesResponse};
use wasm_bindgen::prelude::*;

use crate::error::Result;
use crate::types::{JsAmount, JsBolt11Invoice};

#[wasm_bindgen(js_name = CheckFeesRequest)]
pub struct JsCheckFeesRequest {
    inner: CheckFeesRequest,
}

impl Deref for JsCheckFeesRequest {
    type Target = CheckFeesRequest;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<CheckFeesRequest> for JsCheckFeesRequest {
    fn from(inner: CheckFeesRequest) -> JsCheckFeesRequest {
        JsCheckFeesRequest { inner }
    }
}

#[wasm_bindgen(js_class = CheckFeesRequest)]
impl JsCheckFeesRequest {
    #[wasm_bindgen(constructor)]
    pub fn new(invoice: JsBolt11Invoice) -> Result<JsCheckFeesRequest> {
        Ok(JsCheckFeesRequest {
            inner: CheckFeesRequest {
                pr: invoice.clone(),
            },
        })
    }

    /// Get Amount
    #[wasm_bindgen(getter)]
    pub fn invoice(&self) -> JsBolt11Invoice {
        self.inner.pr.clone().into()
    }
}

#[wasm_bindgen(js_name = CheckFeesResponse)]
pub struct JsCheckFeesResponse {
    inner: CheckFeesResponse,
}

impl Deref for JsCheckFeesResponse {
    type Target = CheckFeesResponse;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<CheckFeesResponse> for JsCheckFeesResponse {
    fn from(inner: CheckFeesResponse) -> JsCheckFeesResponse {
        JsCheckFeesResponse { inner }
    }
}

#[wasm_bindgen(js_class = CheckFeesResponse)]
impl JsCheckFeesResponse {
    #[wasm_bindgen(constructor)]
    pub fn new(amount: JsAmount) -> Result<JsCheckFeesResponse> {
        Ok(JsCheckFeesResponse {
            inner: CheckFeesResponse {
                fee: *amount.deref(),
            },
        })
    }

    /// Get Amount
    #[wasm_bindgen(getter)]
    pub fn amount(&self) -> JsAmount {
        self.inner.fee.into()
    }
}

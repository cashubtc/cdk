use std::ops::Deref;

use cdk::nuts::{RestoreRequest, RestoreResponse};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(js_name = RestoreRequest)]
pub struct JsRestoreRequest {
    inner: RestoreRequest,
}

impl Deref for JsRestoreRequest {
    type Target = RestoreRequest;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<RestoreRequest> for JsRestoreRequest {
    fn from(inner: RestoreRequest) -> JsRestoreRequest {
        JsRestoreRequest { inner }
    }
}

#[wasm_bindgen(js_name = RestoreResponse)]
pub struct JsRestoreResponse {
    inner: RestoreResponse,
}

impl Deref for JsRestoreResponse {
    type Target = RestoreResponse;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<RestoreResponse> for JsRestoreResponse {
    fn from(inner: RestoreResponse) -> JsRestoreResponse {
        JsRestoreResponse { inner }
    }
}

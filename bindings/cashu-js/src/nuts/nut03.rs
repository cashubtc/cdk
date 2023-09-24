use std::ops::Deref;

use cashu::nuts::nut03::RequestMintResponse;
use wasm_bindgen::prelude::*;

use crate::types::JsBolt11Invoice;

#[wasm_bindgen(js_name = RequestMintResponse)]
pub struct JsRequestMintResponse {
    inner: RequestMintResponse,
}

impl Deref for JsRequestMintResponse {
    type Target = RequestMintResponse;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<RequestMintResponse> for JsRequestMintResponse {
    fn from(inner: RequestMintResponse) -> JsRequestMintResponse {
        JsRequestMintResponse { inner }
    }
}

#[wasm_bindgen(js_class = RequestMintResponse)]
impl JsRequestMintResponse {
    #[wasm_bindgen(constructor)]
    pub fn new(pr: JsBolt11Invoice, hash: String) -> JsRequestMintResponse {
        JsRequestMintResponse {
            inner: RequestMintResponse {
                pr: pr.deref().clone(),
                hash,
            },
        }
    }

    /// Get Bolt11 Invoice
    #[wasm_bindgen(getter)]
    pub fn invoice(&self) -> JsBolt11Invoice {
        self.inner.pr.clone().into()
    }

    /// Get Hash
    #[wasm_bindgen(getter)]
    pub fn hash(&self) -> String {
        self.inner.hash.to_string()
    }
}

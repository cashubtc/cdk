use std::ops::Deref;

use cashu::nuts::nut04::{MintRequest, PostMintResponse};
use wasm_bindgen::prelude::*;

use crate::{
    error::{into_err, Result},
    types::JsAmount,
};

#[wasm_bindgen(js_name = MintRequest)]
pub struct JsMintRequest {
    inner: MintRequest,
}

impl Deref for JsMintRequest {
    type Target = MintRequest;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<MintRequest> for JsMintRequest {
    fn from(inner: MintRequest) -> JsMintRequest {
        JsMintRequest { inner }
    }
}

#[wasm_bindgen(js_class = MintRequest)]
impl JsMintRequest {
    /// Try From Base 64 String
    #[wasm_bindgen(constructor)]
    pub fn new(outputs: String) -> Result<JsMintRequest> {
        let outputs = serde_json::from_str(&outputs).map_err(into_err)?;
        Ok(JsMintRequest {
            inner: MintRequest { outputs },
        })
    }

    #[wasm_bindgen(getter)]
    pub fn outputs(&self) -> Result<String> {
        Ok(serde_json::to_string(&self.inner.outputs).map_err(into_err)?)
    }

    #[wasm_bindgen(js_name = totalAmount)]
    pub fn totoal_amount(&self) -> JsAmount {
        self.inner.total_amount().into()
    }
}

#[wasm_bindgen(js_name = PostMintResponse)]
pub struct JsPostMintResponse {
    inner: PostMintResponse,
}

impl Deref for JsPostMintResponse {
    type Target = PostMintResponse;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<PostMintResponse> for JsPostMintResponse {
    fn from(inner: PostMintResponse) -> JsPostMintResponse {
        JsPostMintResponse { inner }
    }
}

#[wasm_bindgen(js_class = PostMintResponse)]
impl JsPostMintResponse {
    /// Try From Base 64 String
    #[wasm_bindgen(constructor)]
    pub fn new(promises: String) -> Result<JsPostMintResponse> {
        let promises = serde_json::from_str(&promises).map_err(into_err)?;
        Ok(JsPostMintResponse {
            inner: PostMintResponse { promises },
        })
    }

    #[wasm_bindgen(getter)]
    pub fn promises(&self) -> Result<String> {
        Ok(serde_json::to_string(&self.inner.promises).map_err(into_err)?)
    }
}

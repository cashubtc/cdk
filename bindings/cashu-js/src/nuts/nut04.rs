use std::ops::Deref;

use cashu::nuts::nut04::{MintBolt11Request, MintBolt11Response};
use wasm_bindgen::prelude::*;

use crate::error::{into_err, Result};
use crate::types::JsAmount;

#[wasm_bindgen(js_name = MintBolt11Request)]
pub struct JsMintBolt11Request {
    inner: MintBolt11Request,
}

impl Deref for JsMintBolt11Request {
    type Target = MintBolt11Request;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<MintBolt11Request> for JsMintBolt11Request {
    fn from(inner: MintBolt11Request) -> JsMintBolt11Request {
        JsMintBolt11Request { inner }
    }
}

#[wasm_bindgen(js_class = MintBolt11Request)]
impl JsMintBolt11Request {
    /// Try From Base 64 String
    #[wasm_bindgen(constructor)]
    pub fn new(quote: String, outputs: JsValue) -> Result<JsMintBolt11Request> {
        let outputs = serde_wasm_bindgen::from_value(outputs).map_err(into_err)?;
        Ok(JsMintBolt11Request {
            inner: MintBolt11Request { quote, outputs },
        })
    }

    #[wasm_bindgen(getter)]
    pub fn outputs(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.outputs).map_err(into_err)
    }

    #[wasm_bindgen(js_name = totalAmount)]
    pub fn totoal_amount(&self) -> JsAmount {
        self.inner.total_amount().into()
    }
}

#[wasm_bindgen(js_name = PostMintResponse)]
pub struct JsMintBolt11Response {
    inner: MintBolt11Response,
}

impl Deref for JsMintBolt11Response {
    type Target = MintBolt11Response;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<MintBolt11Response> for JsMintBolt11Response {
    fn from(inner: MintBolt11Response) -> JsMintBolt11Response {
        JsMintBolt11Response { inner }
    }
}

#[wasm_bindgen(js_class = PostMintResponse)]
impl JsMintBolt11Response {
    /// Try From Base 64 String
    #[wasm_bindgen(constructor)]
    pub fn new(signatures: JsValue) -> Result<JsMintBolt11Response> {
        let signatures = serde_wasm_bindgen::from_value(signatures).map_err(into_err)?;
        Ok(JsMintBolt11Response {
            inner: MintBolt11Response { signatures },
        })
    }

    #[wasm_bindgen(getter)]
    pub fn signatures(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.signatures).map_err(into_err)
    }
}

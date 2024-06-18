use std::ops::Deref;

use cdk::nuts::nut04::{MintBolt11Request, MintBolt11Response, MintMethodSettings, Settings};
use cdk::nuts::{MintQuoteBolt11Request, MintQuoteBolt11Response};
use wasm_bindgen::prelude::*;

use crate::error::{into_err, Result};
use crate::types::JsAmount;

#[wasm_bindgen(js_name = MintQuoteBolt11Request)]
pub struct JsMintQuoteBolt11Request {
    inner: MintQuoteBolt11Request,
}

impl Deref for JsMintQuoteBolt11Request {
    type Target = MintQuoteBolt11Request;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<MintQuoteBolt11Request> for JsMintQuoteBolt11Request {
    fn from(inner: MintQuoteBolt11Request) -> JsMintQuoteBolt11Request {
        JsMintQuoteBolt11Request { inner }
    }
}

#[wasm_bindgen(js_name = MintQuoteBolt11Response)]
pub struct JsMintQuoteBolt11Response {
    inner: MintQuoteBolt11Response,
}

impl Deref for JsMintQuoteBolt11Response {
    type Target = MintQuoteBolt11Response;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<MintQuoteBolt11Response> for JsMintQuoteBolt11Response {
    fn from(inner: MintQuoteBolt11Response) -> JsMintQuoteBolt11Response {
        JsMintQuoteBolt11Response { inner }
    }
}

#[wasm_bindgen(js_class = MintQuoteBolt11Response)]
impl JsMintQuoteBolt11Response {
    #[wasm_bindgen(getter)]
    pub fn state(&self) -> String {
        self.inner.state.to_string()
    }

    #[wasm_bindgen(getter)]
    pub fn quote(&self) -> String {
        self.inner.quote.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn request(&self) -> String {
        self.inner.request.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn expiry(&self) -> Option<u64> {
        self.inner.expiry
    }
}

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
    pub fn total_amount(&self) -> JsAmount {
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

#[wasm_bindgen(js_name = MintMethodSettings)]
pub struct JsMintMethodSettings {
    inner: MintMethodSettings,
}

impl Deref for JsMintMethodSettings {
    type Target = MintMethodSettings;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<MintMethodSettings> for JsMintMethodSettings {
    fn from(inner: MintMethodSettings) -> JsMintMethodSettings {
        JsMintMethodSettings { inner }
    }
}

#[wasm_bindgen(js_name = Settings)]
pub struct JsSettings {
    inner: Settings,
}

impl Deref for JsSettings {
    type Target = Settings;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<Settings> for JsSettings {
    fn from(inner: Settings) -> JsSettings {
        JsSettings { inner }
    }
}

use std::ops::Deref;

use cdk::nuts::{
    MeltBolt11Request, MeltMethodSettings, MeltQuoteBolt11Request, MeltQuoteBolt11Response,
    NUT05Settings,
};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(js_name = MeltQuoteBolt11Request)]
pub struct JsMeltQuoteBolt11Request {
    inner: MeltQuoteBolt11Request,
}

impl Deref for JsMeltQuoteBolt11Request {
    type Target = MeltQuoteBolt11Request;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<MeltQuoteBolt11Request> for JsMeltQuoteBolt11Request {
    fn from(inner: MeltQuoteBolt11Request) -> JsMeltQuoteBolt11Request {
        JsMeltQuoteBolt11Request { inner }
    }
}

#[wasm_bindgen(js_name = MeltQuoteBolt11Response)]
pub struct JsMeltQuoteBolt11Response {
    inner: MeltQuoteBolt11Response,
}

impl Deref for JsMeltQuoteBolt11Response {
    type Target = MeltQuoteBolt11Response;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<MeltQuoteBolt11Response> for JsMeltQuoteBolt11Response {
    fn from(inner: MeltQuoteBolt11Response) -> JsMeltQuoteBolt11Response {
        JsMeltQuoteBolt11Response { inner }
    }
}

#[wasm_bindgen(js_name = MeltBolt11Request)]
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

#[wasm_bindgen(js_name = MeltMethodSettings)]
pub struct JsMeltMethodSettings {
    inner: MeltMethodSettings,
}

impl Deref for JsMeltMethodSettings {
    type Target = MeltMethodSettings;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<MeltMethodSettings> for JsMeltMethodSettings {
    fn from(inner: MeltMethodSettings) -> JsMeltMethodSettings {
        JsMeltMethodSettings { inner }
    }
}

#[wasm_bindgen(js_name = Nut05Settings)]
pub struct JsSettings {
    inner: NUT05Settings,
}

impl Deref for JsSettings {
    type Target = NUT05Settings;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<NUT05Settings> for JsSettings {
    fn from(inner: NUT05Settings) -> JsSettings {
        JsSettings { inner }
    }
}

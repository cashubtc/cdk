use std::ops::Deref;

use cdk::types::MeltQuote;
use wasm_bindgen::prelude::*;

use crate::nuts::JsCurrencyUnit;
use crate::types::JsAmount;

#[wasm_bindgen(js_name = MeltQuote)]
pub struct JsMeltQuote {
    inner: MeltQuote,
}

impl Deref for JsMeltQuote {
    type Target = MeltQuote;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<MeltQuote> for JsMeltQuote {
    fn from(inner: MeltQuote) -> JsMeltQuote {
        JsMeltQuote { inner }
    }
}

#[wasm_bindgen(js_class = MeltQuote)]
impl JsMeltQuote {
    #[wasm_bindgen(getter)]
    pub fn id(&self) -> String {
        self.inner.id.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn unit(&self) -> JsCurrencyUnit {
        self.inner.unit.clone().into()
    }

    #[wasm_bindgen(getter)]
    pub fn amount(&self) -> JsAmount {
        self.inner.amount.into()
    }

    #[wasm_bindgen(getter)]
    pub fn request(&self) -> String {
        self.inner.request.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn fee_reserve(&self) -> JsAmount {
        self.inner.fee_reserve.into()
    }

    #[wasm_bindgen(getter)]
    pub fn state(&self) -> String {
        self.inner.state.to_string()
    }

    #[wasm_bindgen(getter)]
    pub fn expiry(&self) -> u64 {
        self.inner.expiry
    }
}

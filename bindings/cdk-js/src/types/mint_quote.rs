use std::ops::Deref;

use cdk::types::MintQuote;
use wasm_bindgen::prelude::*;

use crate::nuts::JsCurrencyUnit;
use crate::types::JsAmount;

#[wasm_bindgen(js_name = MintQuote)]
pub struct JsMintQuote {
    inner: MintQuote,
}

impl Deref for JsMintQuote {
    type Target = MintQuote;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<MintQuote> for JsMintQuote {
    fn from(inner: MintQuote) -> JsMintQuote {
        JsMintQuote { inner }
    }
}

#[wasm_bindgen(js_class = MintQuote)]
impl JsMintQuote {
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
    pub fn state(&self) -> String {
        self.inner.state.to_string()
    }

    #[wasm_bindgen(getter)]
    pub fn expiry(&self) -> u64 {
        self.inner.expiry
    }
}

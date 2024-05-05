use std::ops::Deref;

use cdk::types::MeltQuote;
use wasm_bindgen::prelude::*;

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

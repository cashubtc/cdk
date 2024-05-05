use std::ops::Deref;

use cdk::types::MintQuote;
use wasm_bindgen::prelude::*;

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

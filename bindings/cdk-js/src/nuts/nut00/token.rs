use std::ops::Deref;

use cdk::nuts::Token;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(js_name = Token)]
pub struct JsToken {
    inner: Token,
}

impl Deref for JsToken {
    type Target = Token;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<Token> for JsToken {
    fn from(inner: Token) -> JsToken {
        JsToken { inner }
    }
}

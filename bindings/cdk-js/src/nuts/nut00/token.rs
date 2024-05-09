use std::ops::Deref;
use std::str::FromStr;

use cdk::nuts::Token;
use wasm_bindgen::prelude::*;

use crate::error::{into_err, Result};

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

#[wasm_bindgen(js_class = Token)]
impl JsToken {
    #[wasm_bindgen(constructor)]
    pub fn new(token: String) -> Result<JsToken> {
        Ok(Self {
            inner: Token::from_str(&token).map_err(into_err)?,
        })
    }
}

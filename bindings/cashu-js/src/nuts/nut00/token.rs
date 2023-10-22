use std::ops::Deref;
use std::str::FromStr;

use cashu::nuts::nut00::wallet::Token;
use cashu::url::UncheckedUrl;
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
    pub fn new(mint: String, proofs: JsValue, memo: Option<String>) -> Result<JsToken> {
        let mint = UncheckedUrl::from_str(&mint).map_err(into_err)?;
        let proofs = serde_wasm_bindgen::from_value(proofs).map_err(into_err)?;
        Ok(Self {
            inner: Token::new(mint, proofs, memo).map_err(into_err)?,
        })
    }

    /// Memo
    #[wasm_bindgen(getter)]
    pub fn memo(&self) -> Option<String> {
        self.inner.memo.clone()
    }

    /// From String
    #[wasm_bindgen(js_name = fromString)]
    pub fn from_string(token: String) -> Result<JsToken> {
        Ok(JsToken {
            inner: Token::from_str(&token).map_err(into_err)?,
        })
    }

    /// As String
    #[wasm_bindgen(js_name = asString)]
    pub fn as_string(&self) -> Result<String> {
        self.inner.convert_to_string().map_err(into_err)
    }
}

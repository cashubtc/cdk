use std::ops::Deref;

use cashu_sdk::wallet::Wallet;
use wasm_bindgen::prelude::*;

use crate::error::{into_err, Result};

#[wasm_bindgen(js_name = Wallet)]
pub struct JsWallet {
    inner: Wallet,
}

impl Deref for JsWallet {
    type Target = Wallet;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<Wallet> for JsWallet {
    fn from(inner: Wallet) -> JsWallet {
        JsWallet { inner }
    }
}

#[wasm_bindgen(js_class = Wallet)]
impl JsWallet {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Result<JsWallet> {
        todo!()
    }
}

use std::ops::Deref;

use cashu::nuts::nut01::SecretKey;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(js_name = PublicKey)]
pub struct JsSecretKey {
    inner: SecretKey,
}

impl Deref for JsSecretKey {
    type Target = SecretKey;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<SecretKey> for JsSecretKey {
    fn from(inner: SecretKey) -> JsSecretKey {
        JsSecretKey { inner }
    }
}

#[wasm_bindgen(js_class = SecretKey)]
impl JsSecretKey {
    /// To Hex
    #[wasm_bindgen(js_name = toHex)]
    pub fn to_hex(&self) -> String {
        self.inner.to_hex()
    }
}

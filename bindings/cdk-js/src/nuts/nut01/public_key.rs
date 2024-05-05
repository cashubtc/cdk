use std::ops::Deref;

use cdk::nuts::nut01::PublicKey;
use wasm_bindgen::prelude::*;

use crate::error::{into_err, Result};

#[wasm_bindgen(js_name = PublicKey)]
pub struct JsPublicKey {
    inner: PublicKey,
}

impl Deref for JsPublicKey {
    type Target = PublicKey;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<PublicKey> for JsPublicKey {
    fn from(inner: PublicKey) -> JsPublicKey {
        JsPublicKey { inner }
    }
}

#[wasm_bindgen(js_class = PublicKey)]
impl JsPublicKey {
    /// From Hex
    #[wasm_bindgen(js_name = fromHex)]
    pub fn from_hex(hex: String) -> Result<JsPublicKey> {
        Ok(Self {
            inner: PublicKey::from_hex(hex).map_err(into_err)?,
        })
    }

    /// To Hex
    #[wasm_bindgen(js_name = toHex)]
    pub fn to_hex(&self) -> String {
        self.inner.to_hex()
    }
}

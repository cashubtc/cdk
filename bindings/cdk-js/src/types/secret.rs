use std::ops::Deref;

use cdk::secret::Secret;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(js_name = Secret)]
pub struct JsSecret {
    inner: Secret,
}

impl Default for JsSecret {
    fn default() -> Self {
        Self::new()
    }
}

impl Deref for JsSecret {
    type Target = Secret;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<Secret> for JsSecret {
    fn from(inner: Secret) -> JsSecret {
        JsSecret { inner }
    }
}

#[wasm_bindgen(js_class = Secret)]
impl JsSecret {
    #[wasm_bindgen(constructor)]
    pub fn new() -> JsSecret {
        Self {
            inner: Secret::generate(),
        }
    }

    /// As Bytes
    #[wasm_bindgen(js_name = asBytes)]
    pub fn as_bytes(&self) -> Vec<u8> {
        self.inner.as_bytes().to_vec()
    }
}

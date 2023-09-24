use std::ops::Deref;

use cashu::secret::Secret;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(js_name = Secret)]
pub struct JsSecret {
    inner: Secret,
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
            inner: Secret::new(),
        }
    }

    /// As Bytes
    #[wasm_bindgen(js_name = asBytes)]
    pub fn as_bytes(&self) -> Vec<u8> {
        self.inner.as_bytes().to_vec()
    }
}

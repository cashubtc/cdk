use std::ops::Deref;

use cdk::types::Melted;
use wasm_bindgen::prelude::*;

use crate::error::Result;

#[wasm_bindgen(js_name = Melted)]
pub struct JsMelted {
    inner: Melted,
}

impl Deref for JsMelted {
    type Target = Melted;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<Melted> for JsMelted {
    fn from(inner: Melted) -> JsMelted {
        JsMelted { inner }
    }
}

#[wasm_bindgen(js_class = Melted)]
impl JsMelted {
    #[wasm_bindgen(getter)]
    pub fn paid(&self) -> String {
        self.inner.state.to_string()
    }

    #[wasm_bindgen(getter)]
    pub fn preimage(&self) -> Option<String> {
        self.inner.preimage.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn change(&self) -> Result<JsValue> {
        Ok(serde_wasm_bindgen::to_value(&self.inner.change)?)
    }
}

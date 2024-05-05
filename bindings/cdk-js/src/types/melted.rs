use std::ops::Deref;

use cdk::types::Melted;
use wasm_bindgen::prelude::*;

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

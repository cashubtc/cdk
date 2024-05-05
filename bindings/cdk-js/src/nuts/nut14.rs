use std::ops::Deref;

use cdk::nuts::HTLCWitness;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(js_name = HTLCWitness)]
pub struct JsHTLCWitness {
    inner: HTLCWitness,
}

impl Deref for JsHTLCWitness {
    type Target = HTLCWitness;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<HTLCWitness> for JsHTLCWitness {
    fn from(inner: HTLCWitness) -> JsHTLCWitness {
        JsHTLCWitness { inner }
    }
}

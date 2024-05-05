use std::ops::Deref;

use cdk::nuts::{Conditions, P2PKWitness};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(js_name = P2PKWitness)]
pub struct JsP2PKWitness {
    inner: P2PKWitness,
}

impl Deref for JsP2PKWitness {
    type Target = P2PKWitness;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<P2PKWitness> for JsP2PKWitness {
    fn from(inner: P2PKWitness) -> JsP2PKWitness {
        JsP2PKWitness { inner }
    }
}

#[wasm_bindgen(js_name = Conditions)]
pub struct JsConditions {
    inner: Conditions,
}

impl Deref for JsConditions {
    type Target = Conditions;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<Conditions> for JsConditions {
    fn from(inner: Conditions) -> JsConditions {
        JsConditions { inner }
    }
}

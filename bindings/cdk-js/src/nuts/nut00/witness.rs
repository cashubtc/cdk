use std::ops::Deref;

use cdk::nuts::Witness;
use wasm_bindgen::prelude::*;

// use crate::nuts::{JsHTLCWitness, JsP2PKWitness};

#[wasm_bindgen(js_name = Witness)]
pub enum JsWitness {
    JsHTLCWitness,
    JsP2PKWitness,
}

impl Deref for JsWitness {
    type Target = Witness;
    fn deref(&self) -> &Self::Target {
        todo!()
    }
}

impl From<Witness> for JsWitness {
    fn from(inner: Witness) -> JsWitness {
        match inner {
            Witness::P2PKWitness(_) => JsWitness::JsP2PKWitness,
            Witness::HTLCWitness(_) => JsWitness::JsHTLCWitness,
        }
    }
}

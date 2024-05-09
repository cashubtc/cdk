use std::ops::Deref;

use cdk::nuts::{HTLCWitness, P2PKWitness, Witness};
use wasm_bindgen::prelude::*;

use crate::error::Result;

#[wasm_bindgen(js_name = Witness)]
pub struct JsWitness {
    inner: Witness,
}

impl Deref for JsWitness {
    type Target = Witness;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<Witness> for JsWitness {
    fn from(inner: Witness) -> JsWitness {
        JsWitness { inner }
    }
}

#[wasm_bindgen(js_class = Witness)]
impl JsWitness {
    #[wasm_bindgen(constructor)]
    pub fn new(preimage: Option<String>, signatures: Option<Vec<String>>) -> Result<JsWitness> {
        match preimage {
            Some(preimage) => Ok(Witness::HTLCWitness(HTLCWitness {
                preimage,
                signatures,
            })
            .into()),
            None => Ok(Witness::P2PKWitness(P2PKWitness {
                signatures: signatures.unwrap(),
            })
            .into()),
        }
    }

    #[wasm_bindgen(getter)]
    pub fn signatures(&self) -> Option<Vec<String>> {
        self.inner.signatures()
    }

    #[wasm_bindgen(getter)]
    pub fn preimage(&self) -> Option<String> {
        self.inner.preimage()
    }
}

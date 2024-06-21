use std::ops::Deref;

use cdk::nuts::{HTLCWitness, SpendingConditions};
use wasm_bindgen::prelude::*;

use super::JsConditions;
use crate::error::{into_err, Result};

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

#[wasm_bindgen(js_name = HTLCSpendingConditions)]
pub struct JsHTLCSpendingConditions {
    inner: SpendingConditions,
}

impl Deref for JsHTLCSpendingConditions {
    type Target = SpendingConditions;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[wasm_bindgen(js_class = HTLCSpendingConditions)]
impl JsHTLCSpendingConditions {
    #[wasm_bindgen(constructor)]
    pub fn new(
        preimage: String,
        conditions: Option<JsConditions>,
    ) -> Result<JsHTLCSpendingConditions> {
        Ok(Self {
            inner: SpendingConditions::new_htlc(preimage, conditions.map(|c| c.deref().clone()))
                .map_err(into_err)?,
        })
    }
}

use std::ops::Deref;

use cashu::Amount;
use wasm_bindgen::prelude::*;

use crate::error::{into_err, Result};

#[wasm_bindgen(js_name = Amount)]
pub struct JsAmount {
    inner: Amount,
}

impl Deref for JsAmount {
    type Target = Amount;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<Amount> for JsAmount {
    fn from(inner: Amount) -> JsAmount {
        JsAmount { inner }
    }
}

#[wasm_bindgen(js_class = Amount)]
impl JsAmount {
    #[wasm_bindgen(constructor)]
    pub fn new(sats: u32) -> Self {
        Self {
            inner: Amount::from_sat(sats as u64),
        }
    }

    /// From Sats
    #[wasm_bindgen(js_name = fromSat)]
    pub fn from_sat(sats: u64) -> Self {
        Self {
            inner: Amount::from_sat(sats),
        }
    }

    /// From Msats
    #[wasm_bindgen(js_name = fromMSat)]
    pub fn from_msat(msats: u64) -> Self {
        Self {
            inner: Amount::from_msat(msats),
        }
    }

    /// Get as sats
    #[wasm_bindgen(js_name = toSat)]
    pub fn to_sat(&self) -> u64 {
        self.inner.to_sat()
    }

    /// Get as msats
    #[wasm_bindgen(js_name = toMSat)]
    pub fn to_msat(&self) -> u64 {
        self.inner.to_msat()
    }

    /// Split amount returns sat vec of sats
    #[wasm_bindgen(js_name = split)]
    pub fn split(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.split()).map_err(into_err)
    }
}

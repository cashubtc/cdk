use std::ops::Deref;
use std::str::FromStr;

use cdk::nuts::{CurrencyUnit, KeySet, KeysResponse, KeysetResponse};
use wasm_bindgen::prelude::*;

use super::JsId;
use crate::error::{into_err, Result};
use crate::nuts::JsKeys;

#[wasm_bindgen(js_name = KeySet)]
pub struct JsKeySet {
    inner: KeySet,
}

impl Deref for JsKeySet {
    type Target = KeySet;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<KeySet> for JsKeySet {
    fn from(inner: KeySet) -> JsKeySet {
        JsKeySet { inner }
    }
}

#[wasm_bindgen(js_class = KeyPair)]
impl JsKeySet {
    /// From Hex
    #[wasm_bindgen(constructor)]
    pub fn new(id: JsId, unit: String, keys: JsKeys) -> JsKeySet {
        Self {
            inner: KeySet {
                id: *id.deref(),
                unit: CurrencyUnit::from_str(&unit).unwrap(),
                keys: keys.deref().clone(),
            },
        }
    }

    #[wasm_bindgen(getter)]
    pub fn id(&self) -> JsId {
        self.inner.id.into()
    }

    #[wasm_bindgen(getter)]
    pub fn keys(&self) -> JsKeys {
        self.inner.keys.clone().into()
    }
}

#[wasm_bindgen(js_name = KeySetsResponse)]
pub struct JsKeySetsResponse {
    inner: KeysetResponse,
}

impl Deref for JsKeySetsResponse {
    type Target = KeysetResponse;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<KeysetResponse> for JsKeySetsResponse {
    fn from(inner: KeysetResponse) -> JsKeySetsResponse {
        JsKeySetsResponse { inner }
    }
}

#[wasm_bindgen(js_class = KeySetsResponse)]
impl JsKeySetsResponse {
    #[wasm_bindgen(constructor)]
    pub fn new(keysets: JsValue) -> Result<JsKeySetsResponse> {
        let response = serde_wasm_bindgen::from_value(keysets).map_err(into_err)?;
        Ok(Self { inner: response })
    }

    /// Get KeySets
    #[wasm_bindgen(getter)]
    pub fn keys(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.keysets).map_err(into_err)
    }
}

#[wasm_bindgen(js_name = KeysResponse)]
pub struct JsKeysResponse {
    inner: KeysResponse,
}

impl Deref for JsKeysResponse {
    type Target = KeysResponse;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<KeysResponse> for JsKeysResponse {
    fn from(inner: KeysResponse) -> JsKeysResponse {
        JsKeysResponse { inner }
    }
}

#[wasm_bindgen(js_class = KeysResponse)]
impl JsKeysResponse {
    #[wasm_bindgen(constructor)]
    pub fn new(keysets: JsValue) -> Result<JsKeysResponse> {
        let response = serde_wasm_bindgen::from_value(keysets).map_err(into_err)?;
        Ok(Self { inner: response })
    }

    /// Get Keys
    #[wasm_bindgen(getter)]
    pub fn keysets(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.keysets).map_err(into_err)
    }
}

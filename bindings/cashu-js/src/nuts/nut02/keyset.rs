use std::ops::Deref;

use cashu::nuts::nut01::Response as KeysResponse;
use cashu::nuts::nut02::{Id, KeySet, Response as KeySetsResponse};
use wasm_bindgen::prelude::*;

use crate::error::{into_err, Result};
use crate::nuts::nut01::JsKeys;

#[wasm_bindgen(js_name = Id)]
pub struct JsId {
    inner: Id,
}

impl Deref for JsId {
    type Target = Id;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<Id> for JsId {
    fn from(inner: Id) -> JsId {
        JsId { inner }
    }
}

#[wasm_bindgen(js_class = Id)]
impl JsId {
    /// Try From Base 64 String
    #[wasm_bindgen(js_name = tryFromBase64)]
    pub fn try_from_base64(id: String) -> Result<JsId> {
        Ok(JsId {
            inner: Id::try_from_base64(&id).map_err(into_err)?,
        })
    }

    /// As String
    #[wasm_bindgen(js_name = asString)]
    pub fn as_string(&self) -> String {
        self.inner.to_string()
    }
}

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
    pub fn new(id: JsId, keys: JsKeys) -> JsKeySet {
        Self {
            inner: KeySet {
                id: *id.deref(),
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
    inner: KeySetsResponse,
}

impl Deref for JsKeySetsResponse {
    type Target = KeySetsResponse;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<KeySetsResponse> for JsKeySetsResponse {
    fn from(inner: KeySetsResponse) -> JsKeySetsResponse {
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
    pub fn keys(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.keys).map_err(into_err)
    }
}

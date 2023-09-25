use std::ops::Deref;

use cashu::nuts::nut02::Id;
use cashu::nuts::nut02::{KeySet, Response};
use wasm_bindgen::prelude::*;

use crate::{
    error::{into_err, Result},
    nuts::nut01::JsKeys,
};

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

#[wasm_bindgen(js_name = KeySetResponse)]
pub struct JsKeyResponse {
    inner: Response,
}

impl Deref for JsKeyResponse {
    type Target = Response;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<Response> for JsKeyResponse {
    fn from(inner: Response) -> JsKeyResponse {
        JsKeyResponse { inner }
    }
}

#[wasm_bindgen(js_class = KeyResponse)]
impl JsKeyResponse {
    /// From Hex
    #[wasm_bindgen(constructor)]
    pub fn new(keysets: String) -> Result<JsKeyResponse> {
        let response = serde_json::from_str(&keysets).map_err(into_err)?;
        Ok(Self { inner: response })
    }

    /// Get Keysets
    #[wasm_bindgen(getter)]
    pub fn keysets(&self) -> Result<String> {
        serde_json::to_string(&self.inner.keysets).map_err(into_err)
    }
}

use std::ops::Deref;

use cashu::nuts::nut01::mint::KeyPair;
use wasm_bindgen::prelude::*;

use super::{JsPublicKey, JsSecretKey};

#[wasm_bindgen(js_name = KeyPair)]
pub struct JsKeyPair {
    inner: KeyPair,
}

impl Deref for JsKeyPair {
    type Target = KeyPair;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<KeyPair> for JsKeyPair {
    fn from(inner: KeyPair) -> JsKeyPair {
        JsKeyPair { inner }
    }
}

#[wasm_bindgen(js_class = KeyPair)]
impl JsKeyPair {
    /// From Hex
    #[wasm_bindgen(js_name = fromSecretKey)]
    pub fn from_secret_key(secret_key: JsSecretKey) -> JsKeyPair {
        Self {
            inner: KeyPair::from_secret_key(secret_key.deref().clone()),
        }
    }

    /// Secret Key
    #[wasm_bindgen(getter)]
    pub fn secret_key(&self) -> JsSecretKey {
        self.inner.secret_key.clone().into()
    }

    /// Public Key
    #[wasm_bindgen(getter)]
    pub fn public_key(&self) -> JsPublicKey {
        self.inner.public_key.clone().into()
    }
}

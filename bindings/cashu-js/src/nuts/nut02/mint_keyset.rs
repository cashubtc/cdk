use std::ops::Deref;

use cashu::nuts::nut02::mint::KeySet;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(js_name = MintKeySet)]
pub struct JsMintKeySet {
    inner: KeySet,
}

impl Deref for JsMintKeySet {
    type Target = KeySet;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<KeySet> for JsMintKeySet {
    fn from(inner: KeySet) -> JsMintKeySet {
        JsMintKeySet { inner }
    }
}

#[wasm_bindgen(js_class = MintKeyPair)]
impl JsMintKeySet {
    /// Generate
    #[wasm_bindgen(constructor)]
    pub fn generate(
        secret: String,
        symbol: String,
        derivation_path: String,
        max_order: u8,
    ) -> JsMintKeySet {
        Self {
            inner: KeySet::generate(secret, symbol, derivation_path, max_order),
        }
    }
}

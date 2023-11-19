use std::ops::Deref;
use std::str::FromStr;

use cashu::nuts::nut02::mint::KeySet;
use cashu::nuts::CurrencyUnit;
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
        unit: String,
        derivation_path: String,
        max_order: u8,
    ) -> JsMintKeySet {
        Self {
            inner: KeySet::generate(
                secret.as_bytes(),
                CurrencyUnit::from_str(&unit).unwrap(),
                &derivation_path,
                max_order,
            ),
        }
    }
}

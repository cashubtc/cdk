use std::ops::Deref;

use cdk::nuts::{PreMint, PreMintSecrets};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(js_name = PreMint)]
pub struct JsPreMint {
    inner: PreMint,
}

impl Deref for JsPreMint {
    type Target = PreMint;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<PreMint> for JsPreMint {
    fn from(inner: PreMint) -> JsPreMint {
        JsPreMint { inner }
    }
}

#[wasm_bindgen(js_name = PreMintSecrets)]
pub struct JsPreMintSecrets {
    inner: PreMintSecrets,
}

impl Deref for JsPreMintSecrets {
    type Target = PreMintSecrets;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<PreMintSecrets> for JsPreMintSecrets {
    fn from(inner: PreMintSecrets) -> JsPreMintSecrets {
        JsPreMintSecrets { inner }
    }
}

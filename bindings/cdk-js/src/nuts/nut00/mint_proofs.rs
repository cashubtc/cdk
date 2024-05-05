use std::ops::Deref;

use cdk::nuts::MintProofs;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(js_name = MintProofs)]
pub struct JsMintProofs {
    inner: MintProofs,
}

impl Deref for JsMintProofs {
    type Target = MintProofs;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<MintProofs> for JsMintProofs {
    fn from(inner: MintProofs) -> JsMintProofs {
        JsMintProofs { inner }
    }
}

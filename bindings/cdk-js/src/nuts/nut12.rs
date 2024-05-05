use std::ops::Deref;

use cdk::nuts::{BlindSignatureDleq, ProofDleq};
use wasm_bindgen::prelude::*;

use crate::nuts::JsSecretKey;

#[wasm_bindgen(js_name = BlindSignatureDleq)]
pub struct JsBlindSignatureDleq {
    inner: BlindSignatureDleq,
}

#[wasm_bindgen(js_class = BlindedSignatureDleq)]
impl JsBlindSignatureDleq {
    #[wasm_bindgen(constructor)]
    pub fn new(e: JsSecretKey, s: JsSecretKey) -> Self {
        Self {
            inner: BlindSignatureDleq {
                e: e.deref().clone(),
                s: s.deref().clone(),
            },
        }
    }

    /// e
    #[wasm_bindgen(getter)]
    pub fn e(&self) -> JsSecretKey {
        self.inner.e.clone().into()
    }

    /// s
    #[wasm_bindgen(getter)]
    pub fn s(&self) -> JsSecretKey {
        self.inner.s.clone().into()
    }
}

impl Deref for JsBlindSignatureDleq {
    type Target = BlindSignatureDleq;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<BlindSignatureDleq> for JsBlindSignatureDleq {
    fn from(inner: BlindSignatureDleq) -> JsBlindSignatureDleq {
        JsBlindSignatureDleq { inner }
    }
}

#[wasm_bindgen(js_name = ProofDleq)]
pub struct JsProofDleq {
    inner: ProofDleq,
}

#[wasm_bindgen(js_class = ProofDleq)]
impl JsProofDleq {
    #[wasm_bindgen(constructor)]
    pub fn new(e: JsSecretKey, s: JsSecretKey, r: JsSecretKey) -> Self {
        Self {
            inner: ProofDleq {
                e: e.deref().clone(),
                s: s.deref().clone(),
                r: r.deref().clone(),
            },
        }
    }

    /// e
    #[wasm_bindgen(getter)]
    pub fn e(&self) -> JsSecretKey {
        self.inner.e.clone().into()
    }

    /// s
    #[wasm_bindgen(getter)]
    pub fn s(&self) -> JsSecretKey {
        self.inner.s.clone().into()
    }

    /// r
    #[wasm_bindgen(getter)]
    pub fn r(&self) -> JsSecretKey {
        self.inner.r.clone().into()
    }
}

impl Deref for JsProofDleq {
    type Target = ProofDleq;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<ProofDleq> for JsProofDleq {
    fn from(inner: ProofDleq) -> JsProofDleq {
        JsProofDleq { inner }
    }
}

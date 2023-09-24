use std::ops::Deref;

use cashu::nuts::nut00::Proof;
use wasm_bindgen::prelude::*;

use crate::{nuts::nut01::JsPublicKey, nuts::nut02::JsId, types::JsAmount, types::JsSecret};

#[wasm_bindgen(js_name = Token)]
pub struct JsProof {
    inner: Proof,
}

impl Deref for JsProof {
    type Target = Proof;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<Proof> for JsProof {
    fn from(inner: Proof) -> JsProof {
        JsProof { inner }
    }
}

#[wasm_bindgen(js_class = Proof)]
impl JsProof {
    #[wasm_bindgen(constructor)]
    pub fn new(amount: JsAmount, secret: JsSecret, c: JsPublicKey, id: Option<JsId>) -> JsProof {
        Self {
            inner: Proof {
                amount: amount.deref().clone(),
                secret: secret.deref().clone(),
                c: c.deref().clone(),
                id: id.map(|i| *i.deref()),
            },
        }
    }

    /// Amount
    #[wasm_bindgen(getter)]
    pub fn amount(&self) -> JsAmount {
        self.inner.amount.into()
    }

    /// Secret
    #[wasm_bindgen(getter)]
    pub fn secret(&self) -> JsSecret {
        self.inner.secret.clone().into()
    }

    /// C
    #[wasm_bindgen(getter)]
    pub fn c(&self) -> JsPublicKey {
        self.inner.c.clone().into()
    }

    /// Id
    #[wasm_bindgen(getter)]
    pub fn id(&self) -> Option<JsId> {
        self.inner.id.map(|id| id.into())
    }
}

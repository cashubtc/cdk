use std::ops::Deref;

use cdk::nuts::Proof;
use wasm_bindgen::prelude::*;

use super::JsWitness;
use crate::nuts::nut01::JsPublicKey;
use crate::nuts::nut02::JsId;
use crate::nuts::nut12::JsProofDleq;
use crate::types::{JsAmount, JsSecret};

#[wasm_bindgen(js_name = Proof)]
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
    pub fn new(
        amount: JsAmount,
        secret: JsSecret,
        c: JsPublicKey,
        keyset_id: JsId,
        witness: Option<JsWitness>,
        dleq: Option<JsProofDleq>,
    ) -> Self {
        Self {
            inner: Proof {
                amount: *amount.deref(),
                secret: secret.deref().clone(),
                c: *c.deref(),
                keyset_id: *keyset_id.deref(),
                witness: witness.map(|w| w.deref().clone()),
                dleq: dleq.map(|d| d.deref().clone()),
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
        self.inner.c.into()
    }

    /// Keyset Id
    #[wasm_bindgen(getter)]
    pub fn keyset_id(&self) -> JsId {
        self.inner.keyset_id.into()
    }
}

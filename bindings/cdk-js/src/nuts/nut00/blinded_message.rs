use std::ops::Deref;

use cdk::nuts::BlindedMessage;
use wasm_bindgen::prelude::*;

use super::JsWitness;
use crate::nuts::{JsId, JsPublicKey};
use crate::types::JsAmount;

#[wasm_bindgen(js_name = BlindedMessage)]
pub struct JsBlindedMessage {
    inner: BlindedMessage,
}

impl Deref for JsBlindedMessage {
    type Target = BlindedMessage;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<BlindedMessage> for JsBlindedMessage {
    fn from(inner: BlindedMessage) -> JsBlindedMessage {
        JsBlindedMessage { inner }
    }
}

#[wasm_bindgen(js_class = BlindedMessage)]
impl JsBlindedMessage {
    #[allow(clippy::new_without_default)]
    #[wasm_bindgen(constructor)]
    pub fn new(
        keyset_id: JsId,
        amount: JsAmount,
        blinded_secret: JsPublicKey,
        witness: Option<JsWitness>,
    ) -> Self {
        Self {
            inner: BlindedMessage {
                keyset_id: *keyset_id.deref(),
                amount: *amount.deref(),
                blinded_secret: *blinded_secret.deref(),
                witness: witness.map(|w| w.deref().clone()),
            },
        }
    }

    /// Keyset Id
    #[wasm_bindgen(getter)]
    pub fn keyset_id(&self) -> JsId {
        self.keyset_id.into()
    }

    /// Amount
    #[wasm_bindgen(getter)]
    pub fn amount(&self) -> JsAmount {
        self.inner.amount.into()
    }

    /// Blinded Secret
    #[wasm_bindgen(getter)]
    pub fn blinded_secret(&self) -> JsPublicKey {
        self.inner.blinded_secret.into()
    }

    /// Witness
    #[wasm_bindgen(getter)]
    pub fn witness(&self) -> Option<JsWitness> {
        self.inner.witness.clone().map(|w| w.into())
    }
}

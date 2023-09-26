use std::ops::Deref;

use cashu_sdk::types::SendProofs;
use wasm_bindgen::prelude::*;

use crate::error::{into_err, Result};

#[wasm_bindgen(js_name = SendProofs)]
pub struct JsSendProofs {
    inner: SendProofs,
}

impl Deref for JsSendProofs {
    type Target = SendProofs;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<SendProofs> for JsSendProofs {
    fn from(inner: SendProofs) -> JsSendProofs {
        JsSendProofs { inner }
    }
}

#[wasm_bindgen(js_class = SendProofs)]
impl JsSendProofs {
    #[wasm_bindgen(constructor)]
    pub fn new(change_proofs: JsValue, send_proofs: JsValue) -> Result<JsSendProofs> {
        let change_proofs = serde_wasm_bindgen::from_value(change_proofs).map_err(into_err)?;
        let send_proofs = serde_wasm_bindgen::from_value(send_proofs).map_err(into_err)?;
        Ok(JsSendProofs {
            inner: SendProofs {
                change_proofs,
                send_proofs,
            },
        })
    }

    /// Get Change Proofs
    #[wasm_bindgen(getter)]
    pub fn change_proofs(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.change_proofs).map_err(into_err)
    }

    /// Get Send Proofs
    #[wasm_bindgen(getter)]
    pub fn send_proofs(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.send_proofs).map_err(into_err)
    }
}

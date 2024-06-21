use std::ops::Deref;
use std::str::FromStr;

use cdk::nuts::{Conditions, P2PKWitness, PublicKey, SigFlag, SpendingConditions};
use wasm_bindgen::prelude::*;

use crate::error::{into_err, Result};

#[wasm_bindgen(js_name = P2PKWitness)]
pub struct JsP2PKWitness {
    inner: P2PKWitness,
}

impl Deref for JsP2PKWitness {
    type Target = P2PKWitness;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<P2PKWitness> for JsP2PKWitness {
    fn from(inner: P2PKWitness) -> JsP2PKWitness {
        JsP2PKWitness { inner }
    }
}

#[wasm_bindgen(js_name = P2PKSpendingConditions)]
pub struct JsP2PKSpendingConditions {
    inner: SpendingConditions,
}

impl Deref for JsP2PKSpendingConditions {
    type Target = SpendingConditions;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[wasm_bindgen(js_class = P2PKSpendingConditions)]
impl JsP2PKSpendingConditions {
    #[wasm_bindgen(constructor)]
    pub fn new(
        pubkey: String,
        conditions: Option<JsConditions>,
    ) -> Result<JsP2PKSpendingConditions> {
        let pubkey = PublicKey::from_str(&pubkey).map_err(into_err)?;
        Ok(Self {
            inner: SpendingConditions::new_p2pk(pubkey, conditions.map(|c| c.deref().clone())),
        })
    }
}

#[wasm_bindgen(js_name = Conditions)]
pub struct JsConditions {
    inner: Conditions,
}

impl Deref for JsConditions {
    type Target = Conditions;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<Conditions> for JsConditions {
    fn from(inner: Conditions) -> JsConditions {
        JsConditions { inner }
    }
}

#[wasm_bindgen(js_class = Conditions)]
impl JsConditions {
    #[wasm_bindgen(constructor)]
    pub fn new(
        locktime: Option<u64>,
        pubkeys: JsValue,
        refund_key: JsValue,
        num_sigs: Option<u64>,
        sig_flag: String,
    ) -> Result<JsConditions> {
        let pubkeys: Result<Vec<PublicKey>, _> = serde_wasm_bindgen::from_value(pubkeys);
        let refund_key: Result<Vec<PublicKey>, _> = serde_wasm_bindgen::from_value(refund_key);

        Ok(Self {
            inner: Conditions::new(
                locktime,
                pubkeys.ok(),
                refund_key.ok(),
                num_sigs,
                Some(SigFlag::from_str(&sig_flag).unwrap_or_default()),
            )
            .map_err(into_err)?,
        })
    }

    #[wasm_bindgen(getter)]
    pub fn locktime(&self) -> Option<u64> {
        self.inner.locktime
    }

    #[wasm_bindgen(getter)]
    pub fn pubkeys(&self) -> Result<JsValue> {
        Ok(serde_wasm_bindgen::to_value(&self.inner.pubkeys)?)
    }

    #[wasm_bindgen(getter)]
    pub fn refund_keys(&self) -> Result<JsValue> {
        Ok(serde_wasm_bindgen::to_value(&self.inner.refund_keys)?)
    }

    #[wasm_bindgen(getter)]
    pub fn num_sigs(&self) -> Option<u64> {
        self.inner.num_sigs
    }

    #[wasm_bindgen(getter)]
    pub fn sig_flag(&self) -> String {
        self.inner.sig_flag.to_string()
    }
}

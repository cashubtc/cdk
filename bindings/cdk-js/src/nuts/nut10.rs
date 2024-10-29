use std::ops::Deref;

use cdk::nuts::{Kind, Nut10Secret, SecretData};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(js_name = Kind)]
pub enum JsKind {
    P2PK,
    HTLC,
}

impl From<Kind> for JsKind {
    fn from(inner: Kind) -> JsKind {
        match inner {
            Kind::P2PK => JsKind::P2PK,
            Kind::HTLC => JsKind::HTLC,
            Kind::DLC => todo!(),
            Kind::SCT => todo!(),
        }
    }
}

impl From<JsKind> for Kind {
    fn from(inner: JsKind) -> Kind {
        match inner {
            JsKind::P2PK => Kind::P2PK,
            JsKind::HTLC => Kind::HTLC,
        }
    }
}

#[wasm_bindgen(js_name = SecretData)]
pub struct JsSecretData {
    inner: SecretData,
}

impl Deref for JsSecretData {
    type Target = SecretData;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<SecretData> for JsSecretData {
    fn from(inner: SecretData) -> JsSecretData {
        JsSecretData { inner }
    }
}

#[wasm_bindgen(js_name = Nut10Secret)]
pub struct JsNut10Secret {
    inner: Nut10Secret,
}

impl Deref for JsNut10Secret {
    type Target = Nut10Secret;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<Nut10Secret> for JsNut10Secret {
    fn from(inner: Nut10Secret) -> JsNut10Secret {
        JsNut10Secret { inner }
    }
}

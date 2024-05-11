use std::ops::Deref;

use cdk::nuts::nut06::{MintInfo, MintVersion};
use cdk::nuts::ContactInfo;
use wasm_bindgen::prelude::*;

use super::nut01::JsPublicKey;
use crate::error::{into_err, Result};

#[wasm_bindgen(js_name = MintVersion)]
pub struct JsMintVersion {
    inner: MintVersion,
}

impl Deref for JsMintVersion {
    type Target = MintVersion;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<MintVersion> for JsMintVersion {
    fn from(inner: MintVersion) -> JsMintVersion {
        JsMintVersion { inner }
    }
}

#[wasm_bindgen(js_class = MintVersion)]
impl JsMintVersion {
    #[wasm_bindgen(constructor)]
    pub fn new(name: String, version: String) -> Result<JsMintVersion> {
        Ok(JsMintVersion {
            inner: MintVersion { name, version },
        })
    }

    /// Get Name
    #[wasm_bindgen(getter)]
    pub fn name(&self) -> String {
        self.inner.name.clone()
    }

    /// Get Version
    #[wasm_bindgen(getter)]
    pub fn version(&self) -> String {
        self.inner.version.clone()
    }
}

#[wasm_bindgen(js_name = MintInfo)]
pub struct JsMintInfo {
    inner: MintInfo,
}

impl Deref for JsMintInfo {
    type Target = MintInfo;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<MintInfo> for JsMintInfo {
    fn from(inner: MintInfo) -> JsMintInfo {
        JsMintInfo { inner }
    }
}

#[wasm_bindgen(js_class = MintInfo)]
impl JsMintInfo {
    #[wasm_bindgen(constructor)]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: Option<String>,
        pubkey: Option<JsPublicKey>,
        version: Option<JsMintVersion>,
        description: Option<String>,
        description_long: Option<String>,
        contact: Option<Vec<JsContactInfo>>,
        nuts: JsValue,
        motd: Option<String>,
    ) -> Result<JsMintInfo> {
        Ok(JsMintInfo {
            inner: MintInfo {
                name,
                pubkey: pubkey.map(|p| *p.deref()),
                version: version.map(|v| v.deref().clone()),
                description,
                description_long,
                contact: contact
                    .map(|contacts| contacts.iter().map(|c| c.deref().clone()).collect()),
                nuts: serde_wasm_bindgen::from_value(nuts).map_err(into_err)?,
                motd,
            },
        })
    }

    /// Get Name
    #[wasm_bindgen(getter)]
    pub fn name(&self) -> Option<String> {
        self.inner.name.clone()
    }

    /// Get Pubkey
    #[wasm_bindgen(getter)]
    pub fn pubkey(&self) -> Option<JsPublicKey> {
        self.inner.pubkey.map(|p| p.into())
    }

    /// Get Version
    #[wasm_bindgen(getter)]
    pub fn version(&self) -> Option<JsMintVersion> {
        self.inner.version.clone().map(|v| v.into())
    }

    /// Get description
    #[wasm_bindgen(getter)]
    pub fn description(&self) -> Option<String> {
        self.inner.description.clone()
    }

    /// Get description long
    #[wasm_bindgen(getter)]
    pub fn description_long(&self) -> Option<String> {
        self.inner.description_long.clone()
    }

    /// Get contact info
    #[wasm_bindgen(getter)]
    pub fn contact(&self) -> Option<Vec<JsContactInfo>> {
        self.inner
            .contact
            .clone()
            .map(|c| c.into_iter().map(|c| c.into()).collect())
    }

    /// Get supported nuts
    #[wasm_bindgen(getter)]
    pub fn nuts(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.nuts).map_err(into_err)
    }

    /// Get motd
    #[wasm_bindgen(getter)]
    pub fn motd(&self) -> Option<String> {
        self.inner.motd.clone()
    }
}

#[wasm_bindgen(js_name = ContactInfo)]
pub struct JsContactInfo {
    inner: ContactInfo,
}

impl Deref for JsContactInfo {
    type Target = ContactInfo;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<ContactInfo> for JsContactInfo {
    fn from(inner: ContactInfo) -> JsContactInfo {
        JsContactInfo { inner }
    }
}

#[wasm_bindgen(js_class = ContactInfo)]
impl JsContactInfo {
    #[wasm_bindgen(constructor)]
    pub fn new(method: String, info: String) -> Result<JsContactInfo> {
        Ok(JsContactInfo {
            inner: ContactInfo { method, info },
        })
    }
    /// Method
    #[wasm_bindgen(getter)]
    pub fn method(&self) -> String {
        self.inner.method.clone()
    }

    /// Info
    #[wasm_bindgen(getter)]
    pub fn info(&self) -> String {
        self.inner.info.clone()
    }
}

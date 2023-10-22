use std::ops::Deref;

use cashu::nuts::nut09::{MintInfo, MintVersion};
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
        contact: JsValue,
        nuts: JsValue,
        motd: Option<String>,
    ) -> Result<JsMintInfo> {
        Ok(JsMintInfo {
            inner: MintInfo {
                name,
                pubkey: pubkey.map(|p| p.deref().clone()),
                version: version.map(|v| v.deref().clone()),
                description,
                description_long,
                contact: serde_wasm_bindgen::from_value(contact).map_err(into_err)?,
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
        self.inner.pubkey.clone().map(|p| p.into())
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

    /// Get contact
    #[wasm_bindgen(getter)]
    pub fn contact(&self) -> Result<JsValue> {
        serde_wasm_bindgen::to_value(&self.inner.contact).map_err(into_err)
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

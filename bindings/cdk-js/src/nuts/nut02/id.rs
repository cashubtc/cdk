use std::ops::Deref;
use std::str::FromStr;

use cdk::nuts::Id;
use wasm_bindgen::prelude::*;

use crate::error::{into_err, Result};

#[wasm_bindgen(js_name = Id)]
pub struct JsId {
    inner: Id,
}

impl Deref for JsId {
    type Target = Id;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<Id> for JsId {
    fn from(inner: Id) -> JsId {
        JsId { inner }
    }
}

#[wasm_bindgen(js_class = Id)]
impl JsId {
    /// Try From Base 64 String
    #[wasm_bindgen(js_name = tryFromBase64)]
    pub fn try_from_base64(id: String) -> Result<JsId> {
        Ok(JsId {
            inner: Id::from_str(&id).map_err(into_err)?,
        })
    }

    /// As String
    #[wasm_bindgen(js_name = asString)]
    pub fn as_string(&self) -> String {
        self.inner.to_string()
    }
}

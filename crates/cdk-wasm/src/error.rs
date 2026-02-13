//! WASM Error types

use cdk::Error as CdkError;
use cdk_common::error::ErrorResponse;
use wasm_bindgen::prelude::*;

/// WASM Error type that wraps CDK errors for cross-language use
#[derive(Debug, thiserror::Error)]
pub enum WasmError {
    /// CDK error with protocol-compliant error code
    #[error("[{code}] {error_message}")]
    Cdk { code: u32, error_message: String },

    /// Internal/infrastructure error
    #[error("{error_message}")]
    Internal { error_message: String },
}

impl WasmError {
    /// Create an internal error from any type that implements ToString
    pub fn internal(msg: impl ToString) -> Self {
        WasmError::Internal {
            error_message: msg.to_string(),
        }
    }

    /// Create a database error (uses Unknown code 50000)
    pub fn database(msg: impl ToString) -> Self {
        WasmError::Cdk {
            code: 50000,
            error_message: msg.to_string(),
        }
    }
}

impl From<CdkError> for WasmError {
    fn from(err: CdkError) -> Self {
        let response = ErrorResponse::from(err);
        WasmError::Cdk {
            code: response.code.to_code() as u32,
            error_message: response.detail,
        }
    }
}

impl From<cdk::amount::Error> for WasmError {
    fn from(err: cdk::amount::Error) -> Self {
        WasmError::internal(err)
    }
}

impl From<cdk::nuts::nut00::Error> for WasmError {
    fn from(err: cdk::nuts::nut00::Error) -> Self {
        WasmError::internal(err)
    }
}

impl From<serde_json::Error> for WasmError {
    fn from(err: serde_json::Error) -> Self {
        WasmError::internal(err)
    }
}

impl From<WasmError> for JsValue {
    fn from(err: WasmError) -> Self {
        JsValue::from_str(&err.to_string())
    }
}

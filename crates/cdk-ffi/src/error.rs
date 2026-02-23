//! FFI Error types

use cdk::Error as CdkError;
use cdk_common::error::ErrorResponse;

/// FFI Error type that wraps CDK errors for cross-language use
///
/// This simplified error type uses protocol-compliant error codes from `ErrorCode`
/// in `cdk-common`, reducing duplication while providing structured error information
/// to FFI consumers.
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Error))]
#[derive(Debug, thiserror::Error)]
pub enum FfiError {
    /// CDK error with protocol-compliant error code
    /// The code corresponds to the Cashu protocol error codes (e.g., 11001, 20001, etc.)
    #[error("[{code}] {error_message}")]
    Cdk {
        /// Error code from the Cashu protocol specification
        code: u32,
        /// Human-readable error message
        error_message: String,
    },

    /// Internal/infrastructure error (no protocol error code)
    /// Used for errors that don't map to Cashu protocol codes
    #[error("{error_message}")]
    Internal {
        /// Human-readable error message
        error_message: String,
    },
}

impl FfiError {
    /// Create an internal error from any type that implements ToString
    pub fn internal(msg: impl ToString) -> Self {
        FfiError::Internal {
            error_message: msg.to_string(),
        }
    }

    /// Create a database error (uses Unknown code 50000)
    pub fn database(msg: impl ToString) -> Self {
        FfiError::Cdk {
            code: 50000,
            error_message: msg.to_string(),
        }
    }
}

impl From<CdkError> for FfiError {
    fn from(err: CdkError) -> Self {
        let response = ErrorResponse::from(err);
        FfiError::Cdk {
            code: response.code.to_code() as u32,
            error_message: response.detail,
        }
    }
}

impl From<cdk::amount::Error> for FfiError {
    fn from(err: cdk::amount::Error) -> Self {
        FfiError::internal(err)
    }
}

impl From<cdk::nuts::nut00::Error> for FfiError {
    fn from(err: cdk::nuts::nut00::Error) -> Self {
        FfiError::internal(err)
    }
}

impl From<serde_json::Error> for FfiError {
    fn from(err: serde_json::Error) -> Self {
        FfiError::internal(err)
    }
}

/// WASM-compatible error wrapper that converts FfiError into a JS Error
#[cfg(feature = "wasm")]
pub struct WasmError(pub FfiError);

#[cfg(feature = "wasm")]
impl From<WasmError> for wasm_bindgen::JsValue {
    fn from(err: WasmError) -> Self {
        js_sys::Error::new(&err.0.to_string()).into()
    }
}

#[cfg(feature = "wasm")]
impl From<FfiError> for WasmError {
    fn from(err: FfiError) -> Self {
        WasmError(err)
    }
}

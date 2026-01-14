//! FFI Error types

use cdk::Error as CdkError;
use cdk_common::error::ErrorResponse;

/// FFI Error type that wraps CDK errors for cross-language use
///
/// This simplified error type uses protocol-compliant error codes from `ErrorCode`
/// in `cdk-common`, reducing duplication while providing structured error information
/// to FFI consumers.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum FfiError {
    /// CDK error with protocol-compliant error code
    /// The code corresponds to the Cashu protocol error codes (e.g., 11001, 20001, etc.)
    #[error("[{code}] {message}")]
    Cdk {
        /// Error code from the Cashu protocol specification
        code: u32,
        /// Human-readable error message
        message: String,
    },

    /// Internal/infrastructure error (no protocol error code)
    /// Used for errors that don't map to Cashu protocol codes
    #[error("{message}")]
    Internal {
        /// Human-readable error message
        message: String,
    },
}

impl FfiError {
    /// Create an internal error from any type that implements ToString
    pub fn internal(msg: impl ToString) -> Self {
        FfiError::Internal {
            message: msg.to_string(),
        }
    }

    /// Create a database error (uses Unknown code 50000)
    pub fn database(msg: impl ToString) -> Self {
        FfiError::Cdk {
            code: 50000,
            message: msg.to_string(),
        }
    }
}

impl From<CdkError> for FfiError {
    fn from(err: CdkError) -> Self {
        let response = ErrorResponse::from(err);
        FfiError::Cdk {
            code: response.code.to_code() as u32,
            message: response.detail,
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

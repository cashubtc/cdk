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

    /// A payment token was created but transport delivery failed.
    ///
    /// The token remains pending. Pass `operation_id` to `Wallet.revoke_send`
    /// to reclaim it if the receiver has not already claimed it.
    #[error(
        "Payment token created but delivery failed for operation {operation_id}: {error_message}"
    )]
    PaymentRequestDeliveryFailed {
        /// Pending send operation that can be passed to `Wallet.revoke_send`.
        operation_id: String,
        /// Transport delivery failure.
        error_message: String,
    },

    /// Seed restore failed after completing at least one batch.
    ///
    /// Pass `partial.next_keyset_indices` to `RestoreStart::KeysetIndices` to
    /// resume scanning without repeating completed batches.
    #[error("Restore interrupted: [{code}] {error_message}")]
    RestoreInterrupted {
        /// Amounts recovered and next per-keyset scan indices before the error.
        partial: crate::types::wallet::Restored,
        /// Error code from the underlying Cashu/CDK error.
        code: u32,
        /// Human-readable message of the underlying error.
        error_message: String,
    },
}

impl FfiError {
    /// Create an internal error from any type that implements ToString
    pub fn internal(msg: impl ToString) -> Self {
        Self::Internal {
            error_message: msg.to_string(),
        }
    }

    /// Create a database error (uses Unknown code 50000)
    pub fn database(msg: impl ToString) -> Self {
        Self::Cdk {
            code: 50000,
            error_message: msg.to_string(),
        }
    }
}

impl From<CdkError> for FfiError {
    fn from(err: CdkError) -> Self {
        let err = match err {
            CdkError::PaymentRequestDeliveryFailed {
                operation_id,
                source,
            } => {
                return Self::PaymentRequestDeliveryFailed {
                    operation_id: operation_id.to_string(),
                    error_message: source.to_string(),
                }
            }
            CdkError::RestoreInterrupted { partial, source } => {
                let response = ErrorResponse::from(*source);
                return Self::RestoreInterrupted {
                    partial: (*partial).into(),
                    code: response.code.to_code() as u32,
                    error_message: response.detail,
                };
            }
            err => err,
        };

        let response = ErrorResponse::from(err);
        Self::Cdk {
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

impl From<cdk::nuts::nut16::Error> for FfiError {
    fn from(err: cdk::nuts::nut16::Error) -> Self {
        FfiError::internal(err)
    }
}

impl From<serde_json::Error> for FfiError {
    fn from(err: serde_json::Error) -> Self {
        FfiError::internal(err)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::str::FromStr;

    use super::*;

    #[test]
    fn restore_interrupted_converts_partial_and_preserves_error_code() {
        let keyset_id = cdk::nuts::Id::from_str("009a1f293253e41e").unwrap();
        let cdk_err = CdkError::RestoreInterrupted {
            partial: Box::new(cdk_common::wallet::Restored {
                spent: cdk::Amount::from(2),
                unspent: cdk::Amount::from(5),
                pending: cdk::Amount::from(1),
                next_keyset_indices: HashMap::from([(keyset_id, 12)]),
            }),
            source: Box::new(CdkError::Timeout),
        };
        let ffi_err = FfiError::from(cdk_err);
        match ffi_err {
            FfiError::RestoreInterrupted {
                partial,
                code,
                error_message,
            } => {
                assert_eq!(partial.spent.value, 2);
                assert_eq!(partial.unspent.value, 5);
                assert_eq!(partial.pending.value, 1);
                assert_eq!(
                    partial.next_keyset_indices,
                    HashMap::from([(keyset_id.to_string(), 12)])
                );
                assert_eq!(code, 50000);
                assert!(!error_message.is_empty());
            }
            other => panic!("expected RestoreInterrupted, got {other:?}"),
        }
    }
}

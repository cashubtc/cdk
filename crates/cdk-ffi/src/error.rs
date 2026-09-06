//! FFI Error types

use cdk::Error as CdkError;
use cdk_common::error::{ErrorResponse, WalletErrorKind as CoreWalletErrorKind};

/// Stable application-level category for wallet errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum WalletErrorKind {
    /// A request or identifier is invalid.
    InvalidInput,
    /// The requested wallet, quote, transaction, or operation does not exist.
    NotFound,
    /// The wallet does not have enough spendable value.
    InsufficientFunds,
    /// A payment failed or remains indeterminate.
    Payment,
    /// The operation conflicts with current persisted state.
    Conflict,
    /// Authentication or authorization is required or failed.
    Authentication,
    /// The mint or selected payment rail does not support the request.
    Unsupported,
    /// Network transport or remote availability failure.
    Network,
    /// Durable wallet storage failed.
    Storage,
    /// An error that does not fit a more specific stable category.
    Internal,
}

/// FFI Error type that wraps CDK errors for cross-language use
///
/// This simplified error type uses protocol-compliant error codes from `ErrorCode`
/// in `cdk-common`, reducing duplication while providing structured error information
/// to FFI consumers.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum FfiError {
    /// Structured wallet error.
    ///
    /// Mint responses retain their Cashu protocol error codes (for example,
    /// 11001 or 20001). Local request validation uses code 40000.
    #[error("[{code}] {error_message}")]
    Cdk {
        /// Stable category for programmatic handling.
        kind: WalletErrorKind,
        /// Error code from the Cashu protocol specification
        code: u32,
        /// Human-readable error message
        error_message: String,
        /// Whether retrying after an external-state change can be useful.
        retryable: bool,
        /// Durable operation associated with the failure, when one was created.
        operation_id: Option<String>,
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
    /// Create a structured local input-validation error.
    pub fn invalid_input(msg: impl ToString) -> Self {
        Self::Cdk {
            kind: WalletErrorKind::InvalidInput,
            code: 40000,
            error_message: msg.to_string(),
            retryable: false,
            operation_id: None,
        }
    }

    /// Create an internal error from any type that implements ToString
    pub fn internal(msg: impl ToString) -> Self {
        Self::Internal {
            error_message: msg.to_string(),
        }
    }

    /// Create a database error (uses Unknown code 50000)
    pub fn database(msg: impl ToString) -> Self {
        Self::Cdk {
            kind: WalletErrorKind::Storage,
            code: 50000,
            error_message: msg.to_string(),
            retryable: false,
            operation_id: None,
        }
    }
}

impl From<CdkError> for FfiError {
    fn from(err: CdkError) -> Self {
        let kind = err.wallet_kind().into();
        let retryable = err.is_retryable();
        let operation_id = match &err {
            CdkError::PaymentRequestDeliveryFailed { operation_id, .. } => {
                Some(operation_id.to_string())
            }
            _ => None,
        };
        let response = ErrorResponse::from(err);
        Self::Cdk {
            kind,
            code: response.code.to_code() as u32,
            error_message: response.detail,
            retryable,
            operation_id,
        }
    }
}

impl From<CoreWalletErrorKind> for WalletErrorKind {
    fn from(value: CoreWalletErrorKind) -> Self {
        match value {
            CoreWalletErrorKind::InvalidInput => Self::InvalidInput,
            CoreWalletErrorKind::NotFound => Self::NotFound,
            CoreWalletErrorKind::InsufficientFunds => Self::InsufficientFunds,
            CoreWalletErrorKind::Payment => Self::Payment,
            CoreWalletErrorKind::Conflict => Self::Conflict,
            CoreWalletErrorKind::Authentication => Self::Authentication,
            CoreWalletErrorKind::Unsupported => Self::Unsupported,
            CoreWalletErrorKind::Network => Self::Network,
            CoreWalletErrorKind::Storage => Self::Storage,
            CoreWalletErrorKind::Internal => Self::Internal,
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
    use super::*;

    #[test]
    fn insufficient_funds_has_a_stable_category() {
        let error = FfiError::from(CdkError::InsufficientFunds);
        assert!(matches!(
            error,
            FfiError::Cdk {
                kind: WalletErrorKind::InsufficientFunds,
                retryable: true,
                ..
            }
        ));
    }

    #[test]
    fn transport_errors_are_retryable() {
        let error = FfiError::from(CdkError::HttpError(None, "offline".to_string()));
        assert!(matches!(
            error,
            FfiError::Cdk {
                kind: WalletErrorKind::Network,
                retryable: true,
                ..
            }
        ));
    }

    #[test]
    fn rejected_http_requests_are_not_blindly_retried() {
        let rejected = FfiError::from(CdkError::HttpError(
            Some(400),
            "request rejected".to_string(),
        ));
        let rate_limited =
            FfiError::from(CdkError::HttpError(Some(429), "rate limited".to_string()));

        assert!(matches!(
            rejected,
            FfiError::Cdk {
                kind: WalletErrorKind::Network,
                retryable: false,
                ..
            }
        ));
        assert!(matches!(
            rate_limited,
            FfiError::Cdk {
                kind: WalletErrorKind::Network,
                retryable: true,
                ..
            }
        ));
    }

    #[test]
    fn workflow_parse_errors_are_invalid_input() {
        let error = FfiError::from(CdkError::InvalidInvoice);
        assert!(matches!(
            error,
            FfiError::Cdk {
                kind: WalletErrorKind::InvalidInput,
                retryable: false,
                ..
            }
        ));
    }

    #[test]
    fn database_reservation_conflicts_are_not_reported_as_storage_failures() {
        let error = FfiError::from(CdkError::Database(
            cdk_common::database::Error::QuoteAlreadyInUse,
        ));
        assert!(matches!(
            error,
            FfiError::Cdk {
                kind: WalletErrorKind::Conflict,
                retryable: true,
                ..
            }
        ));
    }

    #[test]
    fn database_not_found_errors_keep_the_not_found_category() {
        let error = FfiError::from(CdkError::Database(
            cdk_common::database::Error::QuoteNotFound,
        ));
        assert!(matches!(
            error,
            FfiError::Cdk {
                kind: WalletErrorKind::NotFound,
                retryable: false,
                ..
            }
        ));
    }

    #[test]
    fn local_validation_errors_are_structured() {
        let error = FfiError::invalid_input("invalid operation ID");
        assert!(matches!(
            error,
            FfiError::Cdk {
                kind: WalletErrorKind::InvalidInput,
                code: 40000,
                retryable: false,
                ..
            }
        ));
    }
}

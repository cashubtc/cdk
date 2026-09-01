//! FFI Error types

use cdk::Error as CdkError;
use cdk_common::error::ErrorResponse;

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
        }
    }
}

impl From<CdkError> for FfiError {
    fn from(err: CdkError) -> Self {
        let kind = error_kind(&err);
        let retryable = error_is_retryable(&err);
        let response = ErrorResponse::from(err);
        Self::Cdk {
            kind,
            code: response.code.to_code() as u32,
            error_message: response.detail,
            retryable,
        }
    }
}

fn error_is_retryable(error: &CdkError) -> bool {
    if let CdkError::HttpError(status, _) = error {
        return status
            .is_none_or(|status| status >= 500 || matches!(status, 408 | 409 | 425 | 429));
    }

    matches!(
        error,
        CdkError::InsufficientFunds
            | CdkError::PaymentPending
            | CdkError::PendingMeltTimeout { .. }
            | CdkError::UnknownPaymentState
            | CdkError::PaymentRequestDeliveryFailed { .. }
            | CdkError::TransferTimeout { .. }
            | CdkError::TokenPending
            | CdkError::ConcurrentUpdate
            | CdkError::ClearAuthRequired
            | CdkError::BlindAuthRequired
            | CdkError::InsufficientBlindAuthTokens
            | CdkError::Timeout
            | CdkError::SubscriptionError(_)
            | CdkError::CouldNotGetMintInfo
            | CdkError::LightningAddressRequest(_)
            | CdkError::Bip353Resolve(_)
            | CdkError::Database(
                cdk_common::database::Error::QuoteAlreadyInUse
                    | cdk_common::database::Error::Locked
            )
    )
}

fn error_kind(error: &CdkError) -> WalletErrorKind {
    match error {
        CdkError::InsufficientFunds => WalletErrorKind::InsufficientFunds,
        CdkError::UnknownQuote
        | CdkError::UnknownMint { .. }
        | CdkError::TransactionNotFound
        | CdkError::OperationNotFound
        | CdkError::Database(
            cdk_common::database::Error::UnknownQuote
            | cdk_common::database::Error::QuoteNotFound
            | cdk_common::database::Error::ProofNotFound,
        ) => WalletErrorKind::NotFound,
        CdkError::UnknownWallet(_) => WalletErrorKind::NotFound,
        CdkError::PaymentFailed
        | CdkError::PaymentPending
        | CdkError::PendingMeltTimeout { .. }
        | CdkError::UnpaidQuote
        | CdkError::PendingQuote
        | CdkError::UnknownPaymentState
        | CdkError::PaymentRequestDeliveryFailed { .. }
        | CdkError::TransferTimeout { .. } => WalletErrorKind::Payment,
        CdkError::RequestAlreadyPaid
        | CdkError::IssuedQuote
        | CdkError::PaidQuote
        | CdkError::TokenAlreadySpent
        | CdkError::TokenPending
        | CdkError::ConcurrentUpdate
        | CdkError::InvalidOperationState
        | CdkError::ExpiredQuote(_, _)
        | CdkError::DuplicatePaymentId
        | CdkError::Database(
            cdk_common::database::Error::QuoteAlreadyInUse
            | cdk_common::database::Error::Locked
            | cdk_common::database::Error::ProofNotUnspent,
        ) => WalletErrorKind::Conflict,
        CdkError::ClearAuthRequired
        | CdkError::BlindAuthRequired
        | CdkError::ClearAuthFailed
        | CdkError::BlindAuthFailed
        | CdkError::AuthSettingsUndefined
        | CdkError::InsufficientBlindAuthTokens
        | CdkError::AuthLocalstoreUndefined => WalletErrorKind::Authentication,
        CdkError::UnsupportedUnit
        | CdkError::UnsupportedPaymentMethod
        | CdkError::AmountlessInvoiceNotSupported(_, _)
        | CdkError::MppUnitMethodNotSupported(_, _)
        | CdkError::InvoiceDescriptionUnsupported
        | CdkError::MintingDisabled
        | CdkError::MeltingDisabled => WalletErrorKind::Unsupported,
        CdkError::HttpError(_, _)
        | CdkError::Timeout
        | CdkError::SubscriptionError(_)
        | CdkError::CouldNotGetMintInfo
        | CdkError::LightningAddressRequest(_)
        | CdkError::Bip353Resolve(_) => WalletErrorKind::Network,
        CdkError::Database(_) => WalletErrorKind::Storage,
        CdkError::InvalidPaymentRequest
        | CdkError::InvoiceAmountUndefined
        | CdkError::AmountUndefined
        | CdkError::AmountOverflow
        | CdkError::AmountLessNotAllowed
        | CdkError::SplitValuesGreater
        | CdkError::InvalidPaymentMethod
        | CdkError::PaymentMethodRequired
        | CdkError::InvalidInvoice
        | CdkError::Bolt12parse
        | CdkError::AmountOutofLimitRange(_, _, _)
        | CdkError::DuplicateInputs
        | CdkError::DuplicateOutputs
        | CdkError::DuplicateQuoteIds
        | CdkError::BatchSizeExceeded { .. }
        | CdkError::MaxInputsExceeded { .. }
        | CdkError::MaxOutputsExceeded { .. }
        | CdkError::ProofContentTooLarge { .. }
        | CdkError::RequestFieldTooLarge { .. }
        | CdkError::MultipleUnits
        | CdkError::UnitMismatch
        | CdkError::IncorrectMint
        | CdkError::IncorrectWallet(_)
        | CdkError::MultiMintTokenNotSupported
        | CdkError::MaxFeeExceeded
        | CdkError::Bip353Parse(_)
        | CdkError::Bip353NoBolt12Offer
        | CdkError::Bip321Parse(_)
        | CdkError::Bip321Encode(_)
        | CdkError::LightningAddressParse(_)
        | CdkError::InvalidTransactionDirection
        | CdkError::InvalidTransactionStatus
        | CdkError::InvalidTransactionId
        | CdkError::InvalidOperationKind
        | CdkError::KVStoreInvalidKey(_)
        | CdkError::InvalidNut13Options { .. }
        | CdkError::Invoice(_)
        | CdkError::CashuUrl(_)
        | CdkError::AmountError(_)
        | CdkError::NUT00(_) => WalletErrorKind::InvalidInput,
        _ => WalletErrorKind::Internal,
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
    fn portable_parse_errors_are_invalid_input() {
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

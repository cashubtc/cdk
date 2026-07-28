//! Error for payment processor

use thiserror::Error;
use tonic::metadata::MetadataValue;
use tonic::{Code, Status};

const PAYMENT_ERROR_METADATA_KEY: &str = "cdk-payment-error";

/// CDK Payment processor error
#[derive(Debug, Error)]
pub enum Error {
    /// Invalid ID
    #[error("Invalid id")]
    InvalidId,
    /// Invalid payment identifier
    #[error("Invalid payment identifier")]
    InvalidPaymentIdentifier,
    /// Invalid melt options
    #[error("Invalid melt options")]
    InvalidMeltOptions,
    /// Invalid hash
    #[error("Invalid hash")]
    InvalidHash,
    /// Invalid currency unit
    #[error("Invalid currency unit: {0}")]
    InvalidCurrencyUnit(String),
    /// Missing amount field
    #[error("Missing amount field")]
    MissingAmount,
    /// Parse invoice error
    #[error(transparent)]
    Invoice(#[from] lightning_invoice::ParseOrSemanticError),
    /// Hex decode error
    #[error(transparent)]
    Hex(#[from] hex::FromHexError),
    /// JSON error
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// BOLT12 parse error
    #[error("BOLT12 parse error")]
    Bolt12Parse,
    /// NUT00 Error
    #[error(transparent)]
    NUT00(#[from] cdk_common::nuts::nut00::Error),
    /// NUT05 error
    #[error(transparent)]
    NUT05(#[from] cdk_common::nuts::nut05::Error),
    /// Payment error
    #[error(transparent)]
    Payment(#[from] cdk_common::payment::Error),
}

impl From<Error> for Status {
    fn from(error: Error) -> Self {
        match error {
            Error::InvalidId => Status::invalid_argument("Invalid ID"),
            Error::InvalidPaymentIdentifier => {
                Status::invalid_argument("Invalid payment identifier")
            }
            Error::InvalidMeltOptions => Status::invalid_argument("Invalid melt options"),
            Error::InvalidHash => Status::invalid_argument("Invalid hash"),
            Error::InvalidCurrencyUnit(unit) => {
                Status::invalid_argument(format!("Invalid currency unit: {unit}"))
            }
            Error::MissingAmount => Status::invalid_argument("Missing amount field"),
            Error::Invoice(err) => Status::invalid_argument(format!("Invoice error: {err}")),
            Error::Hex(err) => Status::invalid_argument(format!("Hex decode error: {err}")),
            Error::Json(err) => Status::invalid_argument(format!("JSON error: {err}")),
            Error::Bolt12Parse => Status::invalid_argument("BOLT12 parse error"),
            Error::NUT00(err) => Status::internal(format!("NUT00 error: {err}")),
            Error::NUT05(err) => Status::internal(format!("NUT05 error: {err}")),
            Error::Payment(err) => payment_error_to_status(err),
        }
    }
}

impl From<Error> for cdk_common::payment::Error {
    fn from(error: Error) -> Self {
        match error {
            Error::InvalidId => Self::Custom("Invalid ID".to_string()),
            Error::InvalidPaymentIdentifier => {
                Self::Custom("Invalid payment identifier".to_string())
            }
            Error::InvalidMeltOptions => Self::Custom("Invalid melt options".to_string()),
            Error::InvalidHash => Self::Custom("Invalid hash".to_string()),
            Error::InvalidCurrencyUnit(unit) => {
                Self::Custom(format!("Invalid currency unit: {unit}"))
            }
            Error::MissingAmount => Self::Custom("Missing amount field".to_string()),
            Error::Invoice(err) => Self::Custom(format!("Invoice error: {err}")),
            Error::Hex(err) => Self::Custom(format!("Hex decode error: {err}")),
            Error::Json(err) => Self::Custom(format!("JSON error: {err}")),
            Error::Bolt12Parse => Self::Custom("BOLT12 parse error".to_string()),
            Error::NUT00(err) => Self::Custom(format!("NUT00 error: {err}")),
            Error::NUT05(err) => err.into(),
            Error::Payment(err) => err,
        }
    }
}

pub(crate) fn payment_error_to_status(error: cdk_common::payment::Error) -> Status {
    let (code, error_name) = match &error {
        cdk_common::payment::Error::InvoiceAlreadyPaid => {
            (Code::AlreadyExists, "invoice_already_paid")
        }
        cdk_common::payment::Error::InvoicePaymentPending => {
            (Code::Aborted, "invoice_payment_pending")
        }
        cdk_common::payment::Error::UnsupportedUnit => (Code::InvalidArgument, "unsupported_unit"),
        cdk_common::payment::Error::UnsupportedPaymentOption => {
            (Code::Unimplemented, "unsupported_payment_option")
        }
        cdk_common::payment::Error::UnknownPaymentState => {
            (Code::NotFound, "unknown_payment_state")
        }
        cdk_common::payment::Error::AmountMismatch => (Code::InvalidArgument, "amount_mismatch"),
        cdk_common::payment::Error::InvalidExpiry => (Code::InvalidArgument, "invalid_expiry"),
        cdk_common::payment::Error::InvalidHash => (Code::InvalidArgument, "invalid_hash"),
        cdk_common::payment::Error::Serde(_)
        | cdk_common::payment::Error::Parse(_)
        | cdk_common::payment::Error::Amount(_)
        | cdk_common::payment::Error::NUT04(_)
        | cdk_common::payment::Error::NUT05(_)
        | cdk_common::payment::Error::NUT23(_)
        | cdk_common::payment::Error::Hex(_) => (Code::InvalidArgument, "invalid_backend_input"),
        cdk_common::payment::Error::Lightning(_)
        | cdk_common::payment::Error::Onchain(_)
        | cdk_common::payment::Error::Anyhow(_) => (Code::Internal, "backend_error"),
        cdk_common::payment::Error::Custom(_) => (Code::Internal, "custom"),
    };

    let mut status = Status::new(code, error.to_string());
    status.metadata_mut().insert(
        tonic::metadata::MetadataKey::from_static(PAYMENT_ERROR_METADATA_KEY),
        MetadataValue::from_static(error_name),
    );
    status
}

pub(crate) fn payment_error_from_status(status: Status) -> cdk_common::payment::Error {
    let error_name = status
        .metadata()
        .get(PAYMENT_ERROR_METADATA_KEY)
        .and_then(|value| value.to_str().ok());

    match error_name {
        Some("invoice_already_paid") => cdk_common::payment::Error::InvoiceAlreadyPaid,
        Some("invoice_payment_pending") => cdk_common::payment::Error::InvoicePaymentPending,
        Some("unsupported_unit") => cdk_common::payment::Error::UnsupportedUnit,
        Some("unsupported_payment_option") => cdk_common::payment::Error::UnsupportedPaymentOption,
        Some("unknown_payment_state") => cdk_common::payment::Error::UnknownPaymentState,
        Some("amount_mismatch") => cdk_common::payment::Error::AmountMismatch,
        Some("invalid_expiry") => cdk_common::payment::Error::InvalidExpiry,
        Some("invalid_hash") => cdk_common::payment::Error::InvalidHash,
        Some("custom" | "invalid_backend_input" | "backend_error") => {
            cdk_common::payment::Error::Custom(status.message().to_owned())
        }
        _ if status.code() == Code::AlreadyExists || status.message().contains("already paid") => {
            cdk_common::payment::Error::InvoiceAlreadyPaid
        }
        _ if status.code() == Code::Aborted || status.message().contains("pending") => {
            cdk_common::payment::Error::InvoicePaymentPending
        }
        _ => cdk_common::payment::Error::Custom(status.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use cdk_common::payment::Error as PaymentError;

    use super::{payment_error_from_status, payment_error_to_status};

    #[test]
    fn structured_payment_errors_roundtrip_through_status() {
        let errors = [
            PaymentError::InvoiceAlreadyPaid,
            PaymentError::InvoicePaymentPending,
            PaymentError::UnsupportedUnit,
            PaymentError::UnsupportedPaymentOption,
            PaymentError::UnknownPaymentState,
            PaymentError::AmountMismatch,
            PaymentError::InvalidExpiry,
            PaymentError::InvalidHash,
        ];

        for error in errors {
            let expected = error.to_string();
            let roundtrip = payment_error_from_status(payment_error_to_status(error));
            assert_eq!(roundtrip.to_string(), expected);
        }
    }

    #[test]
    fn legacy_payment_statuses_remain_supported() {
        let already_paid =
            payment_error_from_status(tonic::Status::already_exists("Payment already paid"));
        assert!(matches!(already_paid, PaymentError::InvoiceAlreadyPaid));

        let pending = payment_error_from_status(tonic::Status::aborted("Payment pending"));
        assert!(matches!(pending, PaymentError::InvoicePaymentPending));
    }
}

//! Error for payment processor

use thiserror::Error;
use tonic::Status;

/// CDK Payment processor error
#[derive(Debug, Error)]
pub enum Error {
    /// Invalid ID
    #[error("Invalid id")]
    InvalidId,
    /// Invalid payment identifier
    #[error("Invalid payment identifier")]
    InvalidPaymentIdentifier,
    /// Invalid hash
    #[error("Invalid hash")]
    InvalidHash,
    /// Invalid currency unit
    #[error("Invalid currency unit: {0}")]
    InvalidCurrencyUnit(String),
    /// Parse invoice error
    #[error(transparent)]
    Invoice(#[from] lightning_invoice::ParseOrSemanticError),
    /// Hex decode error
    #[error(transparent)]
    Hex(#[from] hex::FromHexError),
    /// BOLT12 parse error
    #[error("BOLT12 parse error")]
    Bolt12Parse,
    /// NUT00 Error
    #[error(transparent)]
    NUT00(#[from] cdk_common::nuts::nut00::Error),
    /// NUT01 Error
    #[error(transparent)]
    NUT01(#[from] cdk_common::nuts::nut01::Error),
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
            Error::InvalidHash => Status::invalid_argument("Invalid hash"),
            Error::InvalidCurrencyUnit(unit) => {
                Status::invalid_argument(format!("Invalid currency unit: {unit}"))
            }
            Error::Invoice(err) => Status::invalid_argument(format!("Invoice error: {err}")),
            Error::Hex(err) => Status::invalid_argument(format!("Hex decode error: {err}")),
            Error::Bolt12Parse => Status::invalid_argument("BOLT12 parse error"),
            Error::NUT00(err) => Status::internal(format!("NUT00 error: {err}")),
            Error::NUT01(err) => Status::internal(format!("NUT01 error: {err}")),
            Error::NUT05(err) => Status::internal(format!("NUT05 error: {err}")),
            Error::Payment(err) => Status::internal(format!("Payment error: {err}")),
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
            Error::InvalidHash => Self::Custom("Invalid hash".to_string()),
            Error::InvalidCurrencyUnit(unit) => {
                Self::Custom(format!("Invalid currency unit: {unit}"))
            }
            Error::Invoice(err) => Self::Custom(format!("Invoice error: {err}")),
            Error::Hex(err) => Self::Custom(format!("Hex decode error: {err}")),
            Error::Bolt12Parse => Self::Custom("BOLT12 parse error".to_string()),
            Error::NUT00(err) => Self::Custom(format!("NUT00 error: {err}")),
            Error::NUT01(err) => Self::Custom(format!("NUT01 error: {err}")),
            Error::NUT05(err) => err.into(),
            Error::Payment(err) => err,
        }
    }
}

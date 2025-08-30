//! LDK Node Errors

use thiserror::Error;

/// LDK Node Error
#[derive(Debug, Error)]
pub enum Error {
    /// LDK Node error
    #[error("LDK Node error: {0}")]
    LdkNode(#[from] ldk_node::NodeError),

    /// LDK Build error
    #[error("LDK Build error: {0}")]
    LdkBuild(#[from] ldk_node::BuildError),

    /// Invalid description
    #[error("Invalid description")]
    InvalidDescription,

    /// Invalid payment hash
    #[error("Invalid payment hash")]
    InvalidPaymentHash,

    /// Invalid payment hash length
    #[error("Invalid payment hash length")]
    InvalidPaymentHashLength,

    /// Invalid payment ID length
    #[error("Invalid payment ID length")]
    InvalidPaymentIdLength,

    /// Unknown invoice amount
    #[error("Unknown invoice amount")]
    UnknownInvoiceAmount,

    /// Could not send bolt11 payment
    #[error("Could not send bolt11 payment")]
    CouldNotSendBolt11,

    /// Could not send bolt11 without amount
    #[error("Could not send bolt11 without amount")]
    CouldNotSendBolt11WithoutAmount,

    /// Payment not found
    #[error("Payment not found")]
    PaymentNotFound,

    /// Could not get amount spent
    #[error("Could not get amount spent")]
    CouldNotGetAmountSpent,

    /// Could not get payment amount
    #[error("Could not get payment amount")]
    CouldNotGetPaymentAmount,

    /// Unexpected payment kind
    #[error("Unexpected payment kind")]
    UnexpectedPaymentKind,

    /// Unsupported payment identifier type
    #[error("Unsupported payment identifier type")]
    UnsupportedPaymentIdentifierType,

    /// Invalid payment direction
    #[error("Invalid payment direction")]
    InvalidPaymentDirection,

    /// Hex decode error
    #[error("Hex decode error: {0}")]
    HexDecode(#[from] cdk_common::util::hex::Error),

    /// JSON error
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Amount conversion error
    #[error("Amount conversion error: {0}")]
    AmountConversion(#[from] cdk_common::amount::Error),

    /// Invalid hex
    #[error("Invalid hex")]
    InvalidHex,
}

impl From<Error> for cdk_common::payment::Error {
    fn from(e: Error) -> Self {
        Self::Lightning(Box::new(e))
    }
}

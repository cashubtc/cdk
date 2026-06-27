//! LDK Server backend errors.

use thiserror::Error;

/// LDK Server backend error.
#[derive(Debug, Error)]
pub enum Error {
    /// LDK Server client construction failed.
    #[error("LDK Server client error: {0}")]
    Client(String),

    /// LDK Server returned an API error.
    #[error("LDK Server API error: {0}")]
    LdkServer(#[from] ldk_server_client::error::LdkServerError),

    /// Unknown invoice amount.
    #[error("Unknown invoice amount")]
    UnknownInvoiceAmount,

    /// Amount overflow.
    #[error("Amount overflow")]
    AmountOverflow,

    /// Invalid payment hash.
    #[error("Invalid payment hash")]
    InvalidPaymentHash,

    /// Invalid payment id.
    #[error("Invalid payment id")]
    InvalidPaymentId,

    /// Payment was not found.
    #[error("Payment not found")]
    PaymentNotFound,

    /// Could not get payment amount.
    #[error("Could not get payment amount")]
    CouldNotGetPaymentAmount,

    /// Could not get amount spent.
    #[error("Could not get amount spent")]
    CouldNotGetAmountSpent,

    /// Payment response had no kind.
    #[error("Payment response had no kind")]
    MissingPaymentKind,

    /// Unexpected payment kind.
    #[error("Unexpected payment kind")]
    UnexpectedPaymentKind,

    /// Unsupported payment identifier type.
    #[error("Unsupported payment identifier type")]
    UnsupportedPaymentIdentifierType,

    /// Invalid payment direction.
    #[error("Invalid payment direction")]
    InvalidPaymentDirection,

    /// Unknown payment direction value.
    #[error("Unknown payment direction value: {0}")]
    UnknownPaymentDirection(i32),

    /// Unknown payment status value.
    #[error("Unknown payment status value: {0}")]
    UnknownPaymentStatus(i32),

    /// Payment scan limit was exceeded.
    #[error("Payment scan limit exceeded after {max_pages} pages")]
    PaymentScanLimitExceeded {
        /// Maximum pages scanned.
        max_pages: u16,
    },

    /// Hex decode error.
    #[error("Hex decode error: {0}")]
    HexDecode(#[from] cdk_common::util::hex::Error),

    /// Amount conversion error.
    #[error("Amount conversion error: {0}")]
    AmountConversion(#[from] cdk_common::amount::Error),
}

impl From<Error> for cdk_common::payment::Error {
    fn from(e: Error) -> Self {
        Self::Lightning(Box::new(e))
    }
}

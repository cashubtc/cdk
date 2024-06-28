//! CDK Wallet Error

use std::num::ParseIntError;

use thiserror::Error;

use crate::cdk_database;
use crate::error::{ErrorCode, ErrorResponse};

/// Wallet Error
#[derive(Debug, Error)]
pub enum Error {
    /// Insufficient Funds
    #[error("Insufficient Funds")]
    InsufficientFunds,
    /// Quote Expired
    #[error("Quote Expired")]
    QuoteExpired,
    /// Unknown Quote
    #[error("Quote Unknown")]
    QuoteUnknown,
    /// Not active keyset
    #[error("No active keyset")]
    NoActiveKeyset,
    /// Invalid DLEQ prood
    #[error("Could not verify Dleq")]
    CouldNotVerifyDleq,
    /// P2PK spending conditions not met
    #[error("P2PK Condition Not met `{0}`")]
    P2PKConditionsNotMet(String),
    /// Invalid Spending Conditions
    #[error("Invalid Spending Conditions: `{0}`")]
    InvalidSpendConditions(String),
    /// Preimage not provided
    #[error("Preimage not provided")]
    PreimageNotProvided,
    /// Unknown Key
    #[error("Unknown Key")]
    UnknownKey,
    /// Spending Locktime not provided
    #[error("Spending condition locktime not provided")]
    LocktimeNotProvided,
    /// Unknown Keyset
    #[error("Url Path segments could not be joined")]
    UrlPathSegments,
    /// Quote not paid
    #[error("Quote not paid")]
    QuoteNotePaid,
    /// Token Already spent error
    #[error("Token Already Spent Error")]
    TokenAlreadySpent,
    /// Unit Not supported
    #[error("Unit not supported for method")]
    UnitNotSupported,
    /// Bolt11 invoice does not have amount
    #[error("Invoice Amount undefined")]
    InvoiceAmountUndefined,
    /// Incorrect quote amount
    #[error("Incorrect quote amount")]
    IncorrectQuoteAmount,
    /// Keyset Not Found
    #[error("Keyset Not Found")]
    KeysetNotFound,
    /// From hex error
    #[error(transparent)]
    ReqwestError(#[from] reqwest::Error),
    ///  Unknown error response
    #[error("Unknown Error response: `{0}`")]
    UnknownErrorResponse(String),
    /// Unknown Wallet
    #[error("Unknown Wallet: `{0}`")]
    UnknownWallet(String),
    /// Unknown Wallet
    #[error("Unknown Wallet: `{0}`")]
    IncorrectWallet(String),
    /// Max Fee Ecxeded
    #[error("Max fee exceeded")]
    MaxFeeExceeded,
    /// CDK Error
    #[error(transparent)]
    Cashu(#[from] crate::error::Error),
    /// Cashu Url Error
    #[error(transparent)]
    CashuUrl(#[from] crate::url::Error),
    /// Database Error
    #[error(transparent)]
    Database(#[from] crate::cdk_database::Error),
    /// NUT00 Error
    #[error(transparent)]
    NUT00(#[from] crate::nuts::nut00::Error),
    /// NUT01 Error
    #[error(transparent)]
    NUT01(#[from] crate::nuts::nut01::Error),
    /// NUT11 Error
    #[error(transparent)]
    NUT11(#[from] crate::nuts::nut11::Error),
    /// NUT12 Error
    #[error(transparent)]
    NUT12(#[from] crate::nuts::nut12::Error),
    /// Parse int
    #[error(transparent)]
    ParseInt(#[from] ParseIntError),
    /// Parse invoice error
    #[error(transparent)]
    Invoice(#[from] lightning_invoice::ParseOrSemanticError),
    /// Serde Error
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
    /// Custom Error
    #[error("`{0}`")]
    Custom(String),
}

impl From<Error> for cdk_database::Error {
    fn from(e: Error) -> Self {
        Self::Database(Box::new(e))
    }
}

impl From<ErrorResponse> for Error {
    fn from(err: ErrorResponse) -> Error {
        match err.code {
            ErrorCode::QuoteNotPaid => Self::QuoteNotePaid,
            ErrorCode::TokenAlreadySpent => Self::TokenAlreadySpent,
            ErrorCode::KeysetNotFound => Self::KeysetNotFound,
            _ => Self::UnknownErrorResponse(err.to_string()),
        }
    }
}

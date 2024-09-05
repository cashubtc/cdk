//! CDK Wallet Error

use std::num::ParseIntError;

use thiserror::Error;

use super::multi_mint_wallet::WalletKey;
use crate::cdk_database;
use crate::error::{ErrorCode, ErrorResponse};
use crate::util::hex;

/// Wallet Error
#[derive(Debug, Error)]
pub enum Error {
    /// Insufficient Funds
    #[error("Insufficient funds")]
    InsufficientFunds,
    /// Quote Expired
    #[error("Quote expired")]
    QuoteExpired,
    /// Unknown Quote
    #[error("Quote unknown")]
    QuoteUnknown,
    /// Not active keyset
    #[error("No active keyset")]
    NoActiveKeyset,
    /// Invalid DLEQ proof
    #[error("Could not verify DLEQ proof")]
    CouldNotVerifyDleq,
    /// P2PK spending conditions not met
    #[error("P2PK condition not met `{0}`")]
    P2PKConditionsNotMet(String),
    /// Invalid Spending Conditions
    #[error("Invalid spending conditions: `{0}`")]
    InvalidSpendConditions(String),
    /// Preimage not provided
    #[error("Preimage not provided")]
    PreimageNotProvided,
    /// Unknown Key
    #[error("Unknown key")]
    UnknownKey,
    /// Spending Locktime not provided
    #[error("Spending condition locktime not provided")]
    LocktimeNotProvided,
    /// Url path segments could not be joined
    #[error("Url path segments could not be joined")]
    UrlPathSegments,
    /// Quote not paid
    #[error("Quote not paid")]
    QuoteNotePaid,
    /// Token Already Spent
    #[error("Token already spent")]
    TokenAlreadySpent,
    /// Unit Not supported
    #[error("Unit not supported for method")]
    UnitNotSupported,
    /// Bolt11 invoice does not have amount
    #[error("Invoice amount undefined")]
    InvoiceAmountUndefined,
    /// Incorrect quote amount
    #[error("Incorrect quote amount")]
    IncorrectQuoteAmount,
    /// Keyset Not Found
    #[error("Keyset not found")]
    KeysetNotFound,
    /// Receive can only be used with tokens from single mint
    #[error("Multiple mint tokens not supported by receive. Please deconstruct the token and use receive with_proof")]
    MultiMintTokenNotSupported,
    /// Incorrect Mint
    /// Token does not match wallet mint
    #[error("Token does not match wallet mint")]
    IncorrectMint,
    /// From hex error
    #[error(transparent)]
    ReqwestError(#[from] reqwest::Error),
    ///  Unknown error response
    #[error("Unknown error response: `{0}`")]
    UnknownErrorResponse(String),
    /// Hex Error
    #[error(transparent)]
    HexError(#[from] hex::Error),
    /// Unknown Wallet
    #[error("Unknown wallet: `{0}`")]
    UnknownWallet(WalletKey),
    /// Incorrect Wallet
    #[error("Incorrect wallet: `{0}`")]
    IncorrectWallet(String),
    /// Max Fee Ecxeded
    #[error("Max fee exceeded")]
    MaxFeeExceeded,
    /// CDK Error
    #[error(transparent)]
    Cashu(#[from] crate::error::Error),
    /// Cashu Url Error
    #[error(transparent)]
    CashuUrl(#[from] crate::mint_url::Error),
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

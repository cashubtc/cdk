//! Errors

use std::string::FromUtf8Error;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::util::hex;

#[derive(Debug, Error)]
pub enum Error {
    /// Mint does not have a key for amount
    #[error("No Key for Amount")]
    AmountKey,
    /// Amount is not what expected
    #[error("Amount miss match")]
    Amount,
    /// Token is already spent
    #[error("Token already spent")]
    TokenSpent,
    /// Token could not be validated
    #[error("Token not verified")]
    TokenNotVerifed,
    /// Bolt11 invoice does not have amount
    #[error("Invoice Amount undefined")]
    InvoiceAmountUndefined,
    /// Proof is missing a required field
    #[error("Proof missing required field")]
    MissingProofField,
    /// No valid point on curve
    #[error("No valid point found")]
    NoValidPoint,
    /// Secp256k1 error
    #[error(transparent)]
    Secp256k1(#[from] bitcoin::secp256k1::Error),
    /// Secret error
    #[error(transparent)]
    Secret(#[from] super::secret::Error),
    /// Bip32 error
    #[error(transparent)]
    Bip32(#[from] bitcoin::bip32::Error),
    /// Parse int error
    #[error(transparent)]
    ParseInt(#[from] std::num::ParseIntError),
    /// Parse Url Error
    #[error(transparent)]
    UrlParseError(#[from] url::ParseError),
    /// Utf8 parse error
    #[error(transparent)]
    Utf8ParseError(#[from] FromUtf8Error),
    /// Serde Json error
    #[error(transparent)]
    SerdeJsonError(#[from] serde_json::Error),
    /// Base64 error
    #[error(transparent)]
    Base64Error(#[from] base64::DecodeError),
    /// From hex error
    #[error(transparent)]
    HexError(#[from] hex::Error),
    #[cfg(feature = "wallet")]
    /// From hex error
    #[error(transparent)]
    ReqwestError(#[from] reqwest::Error),
    /// Nut01 error
    #[error(transparent)]
    NUT01(#[from] crate::nuts::nut01::Error),
    /// NUT02 error
    #[error(transparent)]
    NUT02(#[from] crate::nuts::nut02::Error),
    /// NUT11 Error
    #[error(transparent)]
    NUT11(#[from] crate::nuts::nut11::Error),
    /// Custom error
    #[error("`{0}`")]
    CustomError(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub code: u32,
    pub error: Option<String>,
    pub detail: Option<String>,
}

impl ErrorResponse {
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        if let Ok(res) = serde_json::from_str::<ErrorResponse>(json) {
            Ok(res)
        } else {
            Ok(Self {
                code: 999,
                error: Some(json.to_string()),
                detail: None,
            })
        }
    }
}

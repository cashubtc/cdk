use std::string::FromUtf8Error;

use thiserror::Error;

use crate::nuts::nut01;

#[derive(Debug, Error)]
pub enum Error {
    /// Serde Json error
    #[error(transparent)]
    SerdeJsonError(#[from] serde_json::Error),
    /// Secp256k1 error
    #[error(transparent)]
    Secp256k1(#[from] bitcoin::secp256k1::Error),
    /// NUT01 error
    #[error(transparent)]
    NUT01(#[from] nut01::Error),
    /// Insufficient Funds
    #[error("Insufficient funds")]
    InsufficientFunds,
    /// Utf8 parse error
    #[error(transparent)]
    Utf8ParseError(#[from] FromUtf8Error),
    /// Base64 error
    #[error(transparent)]
    Base64Error(#[from] base64::DecodeError),
    /// Unsupported Token
    #[error("Token unsupported")]
    UnsupportedToken,
    /// Token Requires proofs
    #[error("Proofs Required")]
    ProofsRequired,
    /// Url Parse error
    #[error("Url Parse")]
    UrlParse,
    #[error(transparent)]
    Secret(#[from] crate::secret::Error),
    #[error(transparent)]
    Cashu(#[from] super::Error),
    /// Custom Error message
    #[error("`{0}`")]
    CustomError(String),
}

impl From<crate::url::Error> for Error {
    fn from(_err: crate::url::Error) -> Error {
        Error::UrlParse
    }
}

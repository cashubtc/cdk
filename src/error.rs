use std::string::FromUtf8Error;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    ///  Min req error
    #[error("minreq error: {0}")]
    MinReqError(#[from] minreq::Error),
    /// Parse Url Error
    #[error("minreq error: {0}")]
    UrlParseError(#[from] url::ParseError),
    /// Unsupported Token
    #[error("Unsupported Token")]
    UnsupportedToken,
    /// Utf8 parse error
    #[error("utf8error error: {0}")]
    Utf8ParseError(#[from] FromUtf8Error),
    /// Serde Json error
    #[error("Serde Json error: {0}")]
    SerdeJsonError(#[from] serde_json::Error),
    /// Base64 error
    #[error("Base64 error: {0}")]
    Base64Error(#[from] base64::DecodeError),
    /// Insufficaint Funds
    #[error("Not enough funds")]
    InsufficantFunds,
    #[error("Custom error: {0}")]
    CustomError(String),
    /// From hex error
    #[error("From Hex Error: {0}")]
    HexError(#[from] hex::FromHexError),
    /// From elliptic curve
    #[error("From Elliptic: {0}")]
    EllipticError(#[from] k256::elliptic_curve::Error),
}

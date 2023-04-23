use std::string::FromUtf8Error;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    ///  Min req error
    #[error("minreq error: {0}")]
    MinReqError(#[from] minreq::Error),
    /// Parse Url Error
    #[error("minreq error: {0}")]
    UrlParseError(#[from] url::ParseError),
    /// Secp245k1
    #[error("secp256k1 error: {0}")]
    Secpk256k1Error(#[from] secp256k1::Error),
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
}

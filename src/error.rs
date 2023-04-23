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
}

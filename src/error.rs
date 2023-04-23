#[derive(Debug, thiserror::Error)]
pub enum Error {
    ///  Min req error
    #[error("minreq error: {0}")]
    MinReqError(#[from] minreq::Error),
    /// Parse Url Error
    #[error("minreq error: {0}")]
    UrlParseError(#[from] url::ParseError),
}

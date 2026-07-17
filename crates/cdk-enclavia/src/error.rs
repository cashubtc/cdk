use thiserror::Error;

/// Errors returned while configuring an Enclavia-backed CDK client.
#[derive(Debug, Error)]
pub enum Error {
    /// The configured mint URL could not be parsed.
    #[error("invalid mint URL `{url}`: {source}")]
    InvalidMintUrl {
        /// Mint URL supplied to the connector.
        url: String,
        /// URL parsing error.
        #[source]
        source: url::ParseError,
    },
    /// The mint URL does not use HTTP or HTTPS.
    #[error("unsupported mint URL scheme `{scheme}`; expected http or https")]
    UnsupportedMintScheme {
        /// Unsupported URL scheme.
        scheme: String,
    },
    /// The mint URL contains credentials.
    #[error("mint URL must not contain credentials")]
    MintUrlCredentials,
    /// Establishing or attesting the Enclavia connection failed.
    #[error("could not establish attested Enclavia connection: {0}")]
    Enclavia(#[source] Box<enclavia::Error>),
}

impl From<enclavia::Error> for Error {
    fn from(error: enclavia::Error) -> Self {
        Self::Enclavia(Box::new(error))
    }
}

/// Result type used by this crate.
pub type Result<T> = std::result::Result<T, Error>;

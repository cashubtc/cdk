use std::error::Error as StdError;
use std::fmt;
use std::string::FromUtf8Error;

#[derive(Debug)]
pub enum Error {
    ///  Min req error
    MinReqError(minreq::Error),
    /// Parse Url Error
    UrlParseError(url::ParseError),
    /// Unsupported Token
    UnsupportedToken,
    /// Utf8 parse error
    Utf8ParseError(FromUtf8Error),
    /// Serde Json error
    SerdeJsonError(serde_json::Error),
    /// Base64 error
    Base64Error(base64::DecodeError),
    /// Insufficaint Funds
    InsufficantFunds,
    CustomError(String),
    /// From hex error
    HexError(hex::FromHexError),
    /// From elliptic curve
    EllipticError(k256::elliptic_curve::Error),
    AmountKey,
    Amount,
    TokenSpent,
    TokenNotVerifed,
    OutputOrdering,
    InvoiceAmountUndefined,
    CrabMintError(crate::client::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::MinReqError(err) => write!(f, "{}", err),
            Error::UrlParseError(err) => write!(f, "{}", err),
            Error::UnsupportedToken => write!(f, "Unsuppported Token"),
            Error::Utf8ParseError(err) => write!(f, "{}", err),
            Error::SerdeJsonError(err) => write!(f, "{}", err),
            Error::Base64Error(err) => write!(f, "{}", err),
            Error::InsufficantFunds => write!(f, "Insufficant Funds"),
            Error::CustomError(err) => write!(f, "{}", err),
            Error::HexError(err) => write!(f, "{}", err),
            Error::EllipticError(err) => write!(f, "{}", err),
            Error::AmountKey => write!(f, "No Key for amount"),
            Error::Amount => write!(f, "Amount miss match"),
            Error::TokenSpent => write!(f, "Token Spent"),
            Error::TokenNotVerifed => write!(f, "Token Not Verified"),
            Error::CrabMintError(err) => write!(f, "{}", err),
            Error::OutputOrdering => write!(f, "Output ordering"),
            Error::InvoiceAmountUndefined => write!(f, "Invoice without amount"),
        }
    }
}

impl StdError for Error {}

impl From<minreq::Error> for Error {
    fn from(err: minreq::Error) -> Error {
        Error::MinReqError(err)
    }
}

impl From<url::ParseError> for Error {
    fn from(err: url::ParseError) -> Error {
        Error::UrlParseError(err)
    }
}

impl From<FromUtf8Error> for Error {
    fn from(err: FromUtf8Error) -> Error {
        Error::Utf8ParseError(err)
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Error {
        Error::SerdeJsonError(err)
    }
}

impl From<base64::DecodeError> for Error {
    fn from(err: base64::DecodeError) -> Error {
        Error::Base64Error(err)
    }
}

impl From<hex::FromHexError> for Error {
    fn from(err: hex::FromHexError) -> Error {
        Error::HexError(err)
    }
}

impl From<k256::elliptic_curve::Error> for Error {
    fn from(err: k256::elliptic_curve::Error) -> Error {
        Error::EllipticError(err)
    }
}

impl From<crate::client::Error> for Error {
    fn from(err: crate::client::Error) -> Error {
        Error::CrabMintError(err)
    }
}

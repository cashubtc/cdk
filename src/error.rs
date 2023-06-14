use std::string::FromUtf8Error;

use crate::types::MintError;

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
    CrabMintError(MintError),
}

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

impl From<MintError> for Error {
    fn from(err: MintError) -> Error {
        Error::CrabMintError(err)
    }
}

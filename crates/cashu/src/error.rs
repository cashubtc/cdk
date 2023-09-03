use std::error::Error as StdError;
use std::fmt;
use std::string::FromUtf8Error;

#[derive(Debug)]
pub enum Error {
    /// Parse Url Error
    UrlParseError(url::ParseError),
    /// Utf8 parse error
    Utf8ParseError(FromUtf8Error),
    /// Serde Json error
    SerdeJsonError(serde_json::Error),
    /// Base64 error
    Base64Error(base64::DecodeError),
    CustomError(String),
    /// From hex error
    HexError(hex::FromHexError),
    EllipticCurve(k256::elliptic_curve::Error),
    AmountKey,
    Amount,
    TokenSpent,
    TokenNotVerifed,
    InvoiceAmountUndefined,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::UrlParseError(err) => write!(f, "{}", err),
            Error::Utf8ParseError(err) => write!(f, "{}", err),
            Error::SerdeJsonError(err) => write!(f, "{}", err),
            Error::Base64Error(err) => write!(f, "{}", err),
            Error::CustomError(err) => write!(f, "{}", err),
            Error::HexError(err) => write!(f, "{}", err),
            Error::AmountKey => write!(f, "No Key for amount"),
            Error::Amount => write!(f, "Amount miss match"),
            Error::TokenSpent => write!(f, "Token Spent"),
            Error::TokenNotVerifed => write!(f, "Token Not Verified"),
            Error::InvoiceAmountUndefined => write!(f, "Invoice without amount"),
            Error::EllipticCurve(err) => write!(f, "{}", err.to_string()),
        }
    }
}

impl StdError for Error {}

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
        Error::EllipticCurve(err)
    }
}

#[cfg(feature = "wallet")]
pub mod wallet {
    use std::error::Error as StdError;
    use std::fmt;
    use std::string::FromUtf8Error;

    #[derive(Debug)]
    pub enum Error {
        /// Serde Json error
        SerdeJsonError(serde_json::Error),
        /// From elliptic curve
        EllipticError(k256::elliptic_curve::Error),
        /// Insufficaint Funds
        InsufficantFunds,
        /// Utf8 parse error
        Utf8ParseError(FromUtf8Error),
        /// Base64 error
        Base64Error(base64::DecodeError),
        /// Unsupported Token
        UnsupportedToken,
        /// Token Requires proofs
        ProofsRequired,
        /// Custom Error message
        CustomError(String),
    }

    impl StdError for Error {}

    impl fmt::Display for Error {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Error::CustomError(err) => write!(f, "{}", err),
                Error::InsufficantFunds => write!(f, "Insufficant Funds"),
                Error::Utf8ParseError(err) => write!(f, "{}", err),
                Error::Base64Error(err) => write!(f, "{}", err),
                Error::UnsupportedToken => write!(f, "Unsuppported Token"),
                Error::EllipticError(err) => write!(f, "{}", err),
                Error::SerdeJsonError(err) => write!(f, "{}", err),
                Error::ProofsRequired => write!(f, "Token must have at least one proof",),
            }
        }
    }

    impl From<serde_json::Error> for Error {
        fn from(err: serde_json::Error) -> Error {
            Error::SerdeJsonError(err)
        }
    }

    impl From<k256::elliptic_curve::Error> for Error {
        fn from(err: k256::elliptic_curve::Error) -> Error {
            Error::EllipticError(err)
        }
    }

    impl From<FromUtf8Error> for Error {
        fn from(err: FromUtf8Error) -> Error {
            Error::Utf8ParseError(err)
        }
    }

    impl From<base64::DecodeError> for Error {
        fn from(err: base64::DecodeError) -> Error {
            Error::Base64Error(err)
        }
    }
}

#[cfg(feature = "mint")]
pub mod mint {
    use std::error::Error as StdError;
    use std::fmt;

    #[derive(Debug)]
    pub enum Error {
        AmountKey,
        Amount,
        TokenSpent,
        /// From elliptic curve
        EllipticError(k256::elliptic_curve::Error),
        TokenNotVerifed,
        InvoiceAmountUndefined,
        CustomError(String),
    }

    impl StdError for Error {}

    impl fmt::Display for Error {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Error::AmountKey => write!(f, "No Key for amount"),
                Error::Amount => write!(f, "Amount miss match"),
                Error::TokenSpent => write!(f, "Token Spent"),
                Error::EllipticError(err) => write!(f, "{}", err),
                Error::CustomError(err) => write!(f, "{}", err),
                Error::TokenNotVerifed => write!(f, "Token Not Verified"),
                Error::InvoiceAmountUndefined => write!(f, "Invoice without amount"),
            }
        }
    }

    impl From<k256::elliptic_curve::Error> for Error {
        fn from(err: k256::elliptic_curve::Error) -> Error {
            Error::EllipticError(err)
        }
    }
}

//! NUT-26 Error types

use std::fmt;

/// NUT-26 specific errors
#[derive(Debug)]
pub enum Error {
    /// Invalid bech32m prefix (expected "creqb")
    InvalidPrefix,
    /// Invalid TLV structure
    InvalidTlvStructure,
    /// Invalid UTF-8 in string field
    InvalidUtf8,
    /// Invalid public key
    InvalidPubkey,
    /// Unknown NUT-10 kind
    UnknownKind(u16),
    /// Tag too long (>255 bytes)
    TagTooLong,
    /// Bech32 encoding error
    Bech32Error(bitcoin::bech32::DecodeError),
    /// Base64 decoding error
    Base64DecodeError(bitcoin::base64::DecodeError),
    /// CBOR serialization error
    CborError(ciborium::de::Error<std::io::Error>),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidPrefix => write!(f, "Invalid bech32m prefix, expected 'creqb'"),
            Error::InvalidTlvStructure => write!(f, "Invalid TLV structure"),
            Error::InvalidUtf8 => write!(f, "Invalid UTF-8 encoding in string field"),
            Error::InvalidPubkey => write!(f, "Invalid public key"),
            Error::UnknownKind(kind) => write!(f, "Unknown NUT-10 kind: {}", kind),
            Error::TagTooLong => write!(f, "Tag exceeds 255 byte limit"),
            Error::Bech32Error(e) => write!(f, "Bech32 error: {}", e),
            Error::Base64DecodeError(e) => write!(f, "Base64 decode error: {}", e),
            Error::CborError(e) => write!(f, "CBOR error: {}", e),
        }
    }
}

impl std::error::Error for Error {}

impl From<bitcoin::bech32::DecodeError> for Error {
    fn from(e: bitcoin::bech32::DecodeError) -> Self {
        Error::Bech32Error(e)
    }
}

impl From<bitcoin::base64::DecodeError> for Error {
    fn from(e: bitcoin::base64::DecodeError) -> Self {
        Error::Base64DecodeError(e)
    }
}

impl From<ciborium::de::Error<std::io::Error>> for Error {
    fn from(e: ciborium::de::Error<std::io::Error>) -> Self {
        Error::CborError(e)
    }
}

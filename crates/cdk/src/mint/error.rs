use http::StatusCode;
use thiserror::Error;

use crate::cdk_database;
use crate::error::{ErrorCode, ErrorResponse};

#[derive(Debug, Error)]
pub enum Error {
    /// Unknown Keyset
    #[error("Unknown Keyset")]
    UnknownKeySet,
    /// Inactive Keyset
    #[error("Inactive Keyset")]
    InactiveKeyset,
    #[error("No key for amount")]
    AmountKey,
    #[error("Amount")]
    Amount,
    #[error("Duplicate proofs")]
    DuplicateProofs,
    #[error("Token Already Spent")]
    TokenAlreadySpent,
    #[error("Token Pending")]
    TokenPending,
    #[error("Quote not paid")]
    UnpaidQuote,
    #[error("Unknown quote")]
    UnknownQuote,
    #[error("Unknown secret kind")]
    UnknownSecretKind,
    #[error("Cannot have multiple units")]
    MultipleUnits,
    #[error("Blinded Message is already signed")]
    BlindedMessageAlreadySigned,
    #[error(transparent)]
    Cashu(#[from] crate::error::Error),
    #[error(transparent)]
    Secret(#[from] crate::secret::Error),
    #[error(transparent)]
    NUT00(#[from] crate::nuts::nut00::Error),
    #[error(transparent)]
    NUT11(#[from] crate::nuts::nut11::Error),
    #[error(transparent)]
    Nut12(#[from] crate::nuts::nut12::Error),
    #[error(transparent)]
    Nut14(#[from] crate::nuts::nut14::Error),
    /// Database Error
    #[error(transparent)]
    Database(#[from] cdk_database::Error),
    #[error("`{0}`")]
    Custom(String),
}

impl From<Error> for cdk_database::Error {
    fn from(e: Error) -> Self {
        Self::Database(Box::new(e))
    }
}

impl From<Error> for ErrorResponse {
    fn from(err: Error) -> ErrorResponse {
        match err {
            Error::TokenAlreadySpent => ErrorResponse {
                code: ErrorCode::TokenAlreadySpent,
                error: Some(err.to_string()),
                detail: None,
            },
            _ => ErrorResponse {
                code: ErrorCode::Unknown(9999),
                error: Some(err.to_string()),
                detail: None,
            },
        }
    }
}

impl From<Error> for (StatusCode, ErrorResponse) {
    fn from(err: Error) -> (StatusCode, ErrorResponse) {
        (StatusCode::NOT_FOUND, err.into())
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn test_error_response_enum() {
        let error = Error::TokenAlreadySpent;

        let response: ErrorResponse = error.into();

        let json = serde_json::to_string(&response).unwrap();

        let error_response: ErrorResponse = serde_json::from_str(&json).unwrap();

        assert_eq!(response.code, error_response.code);
    }
}

use cdk_common::database::Error as DatabaseError;
use thiserror::Error;

/// Errors that can occur when interacting with Supabase
#[derive(Debug, Error)]
pub enum Error {
    /// Database error
    #[error(transparent)]
    Database(#[from] DatabaseError),
    /// HTTP request error
    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),
    /// URL parsing error
    #[error(transparent)]
    Url(#[from] url::ParseError),
    /// JSON serialization/deserialization error
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
    /// Supabase-specific error
    #[error("Supabase error: {0}")]
    Supabase(String),
}

impl From<Error> for DatabaseError {
    fn from(e: Error) -> Self {
        match e {
            Error::Database(e) => e,
            Error::Reqwest(e) => DatabaseError::Database(Box::new(e)),
            Error::Url(e) => DatabaseError::Database(Box::new(e)),
            Error::Serde(e) => DatabaseError::Database(Box::new(e)),
            Error::Supabase(e) => DatabaseError::Database(Box::new(std::io::Error::other(e))),
        }
    }
}

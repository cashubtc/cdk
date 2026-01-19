use cdk_common::database::Error as DatabaseError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Database(#[from] DatabaseError),
    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),
    #[error(transparent)]
    Url(#[from] url::ParseError),
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
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

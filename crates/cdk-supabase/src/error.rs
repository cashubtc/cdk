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
    /// Schema version mismatch — the database schema is outdated
    #[error(
        "Schema version mismatch: SDK requires version {required}, \
         database has version {found}. \
         An administrator must run migrations to update the database schema."
    )]
    SchemaMismatch {
        /// The schema version required by this SDK version
        required: u32,
        /// The schema version found in the database
        found: u32,
    },
    /// Schema not initialized — the database has no schema_info table
    #[error(
        "Database schema not initialized. \
         An administrator must run the initial migrations before clients can connect. \
         Use `SupabaseWalletDatabase::get_schema_sql()` to get the required SQL."
    )]
    SchemaNotInitialized,
}

impl From<Error> for DatabaseError {
    fn from(e: Error) -> Self {
        match e {
            Error::Database(e) => e,
            Error::Reqwest(e) => DatabaseError::Database(Box::new(e)),
            Error::Url(e) => DatabaseError::Database(Box::new(e)),
            Error::Serde(e) => DatabaseError::Database(Box::new(e)),
            Error::Supabase(msg) => DatabaseError::Database(Box::new(std::io::Error::other(msg))),
            Error::SchemaMismatch { required, found } => {
                DatabaseError::Database(Box::new(std::io::Error::other(format!(
                    "Schema version mismatch: SDK requires version {required}, \
                     database has version {found}. \
                     An administrator must run migrations to update the database schema."
                ))))
            }
            Error::SchemaNotInitialized => {
                DatabaseError::Database(Box::new(std::io::Error::other(
                    "Database schema not initialized. \
                     An administrator must run the initial migrations before clients can connect.",
                )))
            }
        }
    }
}

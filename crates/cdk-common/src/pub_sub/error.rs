//! Error types for the pub-sub module.

use tokio::sync::mpsc::error::TrySendError;

#[derive(thiserror::Error, Debug)]
/// Error
pub enum Error {
    /// No subscription found
    #[error("Subscription not found")]
    NoSubscription,

    /// Parsing error
    #[error("Parsing Error {0}")]
    ParsingError(String),

    /// Internal error
    #[error("Internal")]
    Internal(Box<dyn std::error::Error + Send + Sync>),

    /// Internal error
    #[error("Internal error {0}")]
    InternalStr(String),

    /// Not supported
    #[error("Not supported")]
    NotSupported,

    /// Channel is full
    #[error("Channel is full")]
    ChannelFull,

    /// Channel is closed
    #[error("Channel is close")]
    ChannelClosed,
}

impl<T> From<TrySendError<T>> for Error {
    fn from(value: TrySendError<T>) -> Self {
        match value {
            TrySendError::Closed(_) => Error::ChannelClosed,
            TrySendError::Full(_) => Error::ChannelFull,
        }
    }
}

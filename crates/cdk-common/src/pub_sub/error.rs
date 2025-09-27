#[derive(thiserror::Error, Debug)]
/// Error
pub enum Error {
    /// Poison locked
    #[error("Poisoned lock")]
    Poison,

    /// Already subscribed
    #[error("Already subscribed")]
    AlreadySubscribed,

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
}

#[derive(thiserror::Error, Debug)]
/// Error
pub enum Error {
    /// Poison locked
    #[error("Poisoned lock")]
    Poison,

    /// Already subscribed
    #[error("Already subscribed")]
    AlreadySubscribed,

    /// Parsing error
    #[error("Parsing Error {0}")]
    ParsingError(String),
}

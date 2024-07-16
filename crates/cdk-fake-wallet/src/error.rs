use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    /// Invoice amount not defined
    #[error("Unknown invoice amount")]
    UnknownInvoiceAmount,
    /// Unknown invoice
    #[error("Unknown invoice")]
    UnknownInvoice,
    #[error("`{0}`")]
    Custom(String),
}

impl From<Error> for cdk::cdk_lightning::Error {
    fn from(e: Error) -> Self {
        Self::Lightning(Box::new(e))
    }
}

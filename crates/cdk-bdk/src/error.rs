use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    /// Invoice amount not defined
    #[error("Unknown invoice amount")]
    UnknownInvoiceAmount,
    /// Wrong CLN response
    #[error("Wrong cln response")]
    WrongClnResponse,
    /// Unknown invoice
    #[error("Unknown invoice")]
    UnknownInvoice,
    #[error(transparent)]
    Anyhow(#[from] anyhow::Error),
    /// Esplora client error
    #[error(transparent)]
    EsploraClient(#[from] bdk_esplora::esplora_client::Error),
}

impl From<Error> for cdk::cdk_onchain::Error {
    fn from(e: Error) -> Self {
        Self::Oncahin(Box::new(e))
    }
}

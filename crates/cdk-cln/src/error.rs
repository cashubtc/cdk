//! CLN Errors

use thiserror::Error;

/// CLN Error
#[derive(Debug, Error)]
pub enum Error {
    /// Invoice amount not defined
    #[error("Unknown invoice amount")]
    UnknownInvoiceAmount,
    /// Wrong CLN response
    #[error("Wrong CLN response")]
    WrongClnResponse,
    /// Unknown invoice
    #[error("Unknown invoice")]
    UnknownInvoice,
    /// Invalid payment hash
    #[error("Invalid hash")]
    InvalidHash,
    /// Cln Error
    #[error(transparent)]
    Cln(#[from] cln_rpc::Error),
    /// Cln Rpc Error
    #[error(transparent)]
    ClnRpc(#[from] cln_rpc::RpcError),
    /// Amount Error
    #[error(transparent)]
    Amount(#[from] cdk_common::amount::Error),
    /// UTF-8 Error
    #[error(transparent)]
    Utf8(#[from] std::string::FromUtf8Error),
    /// Bolt12 Error
    #[error("Bolt12 error: {0}")]
    Bolt12(String),
    /// Database Error
    #[error("Database error: {0}")]
    Database(String),
}

impl From<Error> for cdk_common::payment::Error {
    fn from(e: Error) -> Self {
        Self::Lightning(Box::new(e))
    }
}

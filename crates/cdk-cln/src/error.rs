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
    /// Invalid payment hash
    #[error("Invalid payment hash")]
    InvalidPaymentHash,
    /// Cln Error
    #[error(transparent)]
    Cln(#[from] cln_rpc::Error),
    /// Cln Rpc Error
    #[error(transparent)]
    ClnRpc(#[from] cln_rpc::RpcError),
    /// Amount Error
    #[error(transparent)]
    Amount(#[from] cdk_common::amount::Error),
    /// Utf8 parse error
    #[error(transparent)]
    Utf8ParseError(#[from] std::string::FromUtf8Error),
}

impl From<Error> for cdk_common::payment::Error {
    fn from(e: Error) -> Self {
        Self::Lightning(Box::new(e))
    }
}

//! CLN Errors

use thiserror::Error;
use tokio::sync::mpsc::error::SendError;
use tokio::sync::oneshot::error::RecvError;

use crate::connection::Request;

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
    /// Connection failed after max retries
    #[error("Failed to establish connection after maximum retries")]
    ConnectionFailed,
    /// Send error
    #[error("Failed to send request: {0}")]
    SendError(#[from] SendError<Request>),
    /// Receive error
    #[error("Failed to receive response: {0}")]
    ReceiveError(#[from] RecvError),
    /// Cln Error
    #[error(transparent)]
    Cln(#[from] cln_rpc::Error),
    /// Cln Rpc Error
    #[error(transparent)]
    ClnRpc(#[from] cln_rpc::RpcError),
    /// Amount Error
    #[error(transparent)]
    Amount(#[from] cdk_common::amount::Error),
}

impl From<Error> for cdk_common::payment::Error {
    fn from(error: Error) -> Self {
        Self::Lightning(Box::new(error))
    }
}

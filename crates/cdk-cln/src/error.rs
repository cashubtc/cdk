use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Wrong cln response")]
    WrongClnResponse,
    /// Cln Error
    #[error(transparent)]
    Cln(#[from] cln_rpc::Error),
    /// Cln Rpc Error
    #[error(transparent)]
    ClnRpc(#[from] cln_rpc::RpcError),
    #[error("`{0}`")]
    Custom(String),
}

impl From<Error> for cdk::cdk_lightning::Error {
    fn from(e: Error) -> Self {
        Self::Lightning(Box::new(e))
    }
}

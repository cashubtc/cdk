use thiserror::Error;

use crate::cdk_mint_rpc::cdk_mint_client::CdkMintClient;
use crate::cdk_mint_rpc::cdk_mint_server::CdkMint;

/// Error
#[derive(Debug, Error)]
pub enum Error {
    /// Transport error
    #[error(transparent)]
    Transport(#[from] tonic::transport::Error),
}

pub struct MintRPCClient {
    inner: CdkMintClient<tonic::transport::Channel>,
}

impl MintRPCClient {
    pub async fn new(url: String) -> Result<Self, Error> {
        Ok(Self {
            inner: CdkMintClient::connect(url).await?,
        })
    }
}

#[tonic::async_trait]
impl CdkMint for MintRPCClient {}

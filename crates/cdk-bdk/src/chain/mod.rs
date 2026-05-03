use bdk_wallet::bitcoin::Transaction;
use tokio_util::sync::CancellationToken;

use crate::error::Error;

#[cfg(feature = "bitcoin-rpc")]
pub mod bitcoin_rpc;
#[cfg(feature = "esplora")]
pub mod esplora;

/// Configuration for connecting to Bitcoin RPC
#[derive(Debug, Clone)]
pub struct BitcoinRpcConfig {
    /// Bitcoin RPC server hostname or IP address
    pub host: String,
    /// Bitcoin RPC server port number
    pub port: u16,
    /// Username for Bitcoin RPC authentication
    pub user: String,
    /// Password for Bitcoin RPC authentication
    pub password: String,
}

/// Configuration for connecting to Esplora
#[derive(Debug, Clone)]
pub struct EsploraConfig {
    /// URL of the Esplora server endpoint
    pub url: String,
    /// Number of parallel requests to use during sync
    pub parallel_requests: usize,
}

/// Source of blockchain data for the BDK wallet
#[derive(Debug, Clone)]
pub enum ChainSource {
    /// Use an Esplora server for blockchain data
    #[cfg(feature = "esplora")]
    Esplora(EsploraConfig),
    /// Use Bitcoin Core RPC for blockchain data
    #[cfg(feature = "bitcoin-rpc")]
    BitcoinRpc(BitcoinRpcConfig),
}

impl ChainSource {
    pub async fn sync_wallet(
        &self,
        cdk_bdk: &crate::CdkBdk,
        cancel_token: CancellationToken,
    ) -> Result<(), Error> {
        match self {
            #[cfg(feature = "esplora")]
            ChainSource::Esplora(config) => {
                esplora::sync_esplora(cdk_bdk, config, cancel_token).await
            }
            #[cfg(feature = "bitcoin-rpc")]
            ChainSource::BitcoinRpc(config) => {
                bitcoin_rpc::sync_bitcoin_rpc(cdk_bdk, config, cancel_token).await
            }
            #[allow(unreachable_patterns)]
            _ => unreachable!("ChainSource must have at least one feature enabled"),
        }
    }

    pub async fn broadcast(&self, tx: Transaction) -> Result<(), Error> {
        match self {
            #[cfg(feature = "esplora")]
            ChainSource::Esplora(config) => esplora::broadcast_esplora(config, tx).await,
            #[cfg(feature = "bitcoin-rpc")]
            ChainSource::BitcoinRpc(config) => bitcoin_rpc::broadcast_bitcoin_rpc(config, tx).await,
            #[allow(unreachable_patterns)]
            _ => unreachable!("ChainSource must have at least one feature enabled"),
        }
    }

    pub async fn fetch_fee_rate(&self, target_blocks: u16) -> Result<f64, Error> {
        match self {
            #[cfg(feature = "esplora")]
            ChainSource::Esplora(config) => {
                esplora::fetch_fee_rate_esplora(config, target_blocks).await
            }
            #[cfg(feature = "bitcoin-rpc")]
            ChainSource::BitcoinRpc(config) => {
                bitcoin_rpc::fetch_fee_rate_bitcoin_rpc(config, target_blocks).await
            }
            #[allow(unreachable_patterns)]
            _ => unreachable!("ChainSource must have at least one feature enabled"),
        }
    }
}

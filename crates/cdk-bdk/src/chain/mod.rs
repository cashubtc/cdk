use bdk_wallet::bitcoin::Transaction;
use tokio_util::sync::CancellationToken;

use crate::error::Error;

#[cfg(feature = "bitcoin-rpc")]
pub mod bitcoin_rpc;
#[cfg(feature = "electrum")]
pub mod electrum;
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

/// Configuration for connecting to Electrum
#[derive(Debug, Clone)]
pub struct ElectrumConfig {
    /// URL of the Electrum server endpoint
    pub url: String,
    /// Number of scripts to request in each Electrum batch
    pub batch_size: usize,
}

/// Source of blockchain data for the BDK wallet
#[derive(Debug, Clone)]
pub enum ChainSource {
    /// Use an Esplora server for blockchain data
    #[cfg(feature = "esplora")]
    Esplora(EsploraConfig),
    /// Use an Electrum server for blockchain data
    #[cfg(feature = "electrum")]
    Electrum(ElectrumConfig),
    /// Use Bitcoin Core RPC for blockchain data
    #[cfg(feature = "bitcoin-rpc")]
    BitcoinRpc(BitcoinRpcConfig),
}

/// Classified result of submitting a transaction to a chain backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BroadcastOutcome {
    /// Backend accepted the transaction.
    Accepted,
    /// Backend already knows the transaction; this is success-equivalent.
    AlreadyKnown,
}

/// Classification for broadcast errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BroadcastErrorKind {
    /// Deterministic backend rejection.
    Rejected,
    /// Network or upstream failure expected to resolve on retry.
    Transient,
    /// Ambiguous or unrecognized error; retry conservatively.
    Unknown,
}

/// A classified broadcast failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BroadcastFailure {
    /// Failure class.
    pub kind: BroadcastErrorKind,
    /// Human-readable backend error.
    pub message: String,
}

impl BroadcastFailure {
    pub(crate) fn new(kind: BroadcastErrorKind, message: String) -> Self {
        Self { kind, message }
    }
}

impl ChainSource {
    pub(crate) fn validate(&self) -> Result<(), Error> {
        match self {
            #[cfg(feature = "electrum")]
            Self::Electrum(config) if config.batch_size == 0 => {
                return Err(Error::InvalidConfig(
                    "Electrum batch_size must be greater than zero".to_string(),
                ));
            }
            #[allow(unreachable_patterns)]
            _ => {}
        }

        Ok(())
    }

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
            #[cfg(feature = "electrum")]
            ChainSource::Electrum(config) => {
                electrum::sync_electrum(cdk_bdk, config, cancel_token).await
            }
            #[cfg(feature = "bitcoin-rpc")]
            ChainSource::BitcoinRpc(config) => {
                bitcoin_rpc::sync_bitcoin_rpc(cdk_bdk, config, cancel_token).await
            }
            #[allow(unreachable_patterns)]
            _ => unreachable!("ChainSource must have at least one feature enabled"),
        }
    }

    pub(crate) async fn broadcast(
        &self,
        tx: Transaction,
    ) -> Result<BroadcastOutcome, BroadcastFailure> {
        match self {
            #[cfg(feature = "esplora")]
            ChainSource::Esplora(config) => esplora::broadcast_esplora(config, tx).await,
            #[cfg(feature = "electrum")]
            ChainSource::Electrum(config) => electrum::broadcast_electrum(config, tx).await,
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
            #[cfg(feature = "electrum")]
            ChainSource::Electrum(config) => {
                electrum::fetch_fee_rate_electrum(config, target_blocks).await
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

#[cfg(all(test, feature = "electrum"))]
mod tests {
    use super::*;

    #[test]
    fn rejects_zero_electrum_batch_size() {
        let chain_source = ChainSource::Electrum(ElectrumConfig {
            url: "tcp://127.0.0.1:50001".to_string(),
            batch_size: 0,
        });

        let error = chain_source
            .validate()
            .expect_err("zero Electrum batch size should fail");

        assert!(matches!(error, Error::InvalidConfig(_)));
    }
}

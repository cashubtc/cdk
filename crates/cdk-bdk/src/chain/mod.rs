use core::fmt;

use bdk_wallet::bitcoin::Transaction;
use bdk_wallet::chain::BlockId;
use cdk_common::redact::url_for_logs;
use tokio_util::sync::CancellationToken;

use crate::error::Error;

#[cfg(feature = "bitcoin-rpc")]
pub mod bitcoin_rpc;
#[cfg(feature = "electrum")]
pub mod electrum;
#[cfg(feature = "esplora")]
pub mod esplora;

/// Configuration for connecting to Bitcoin RPC
#[derive(Clone)]
pub struct BitcoinRpcConfig {
    /// Bitcoin RPC server hostname or IP address
    pub host: String,
    /// Bitcoin RPC server port number
    pub port: u16,
    /// Username for Bitcoin RPC authentication
    pub user: String,
    /// Password for Bitcoin RPC authentication
    pub password: String,
    /// Optional wallet birthday height used when creating a fresh wallet.
    ///
    /// If unset, a fresh wallet starts at the current Bitcoin Core tip. Set
    /// this when restoring a wallet from seed to scan from a known birthday
    /// height. Existing wallets are never rewound.
    pub wallet_rescan_from_height: Option<u32>,
}

impl fmt::Debug for BitcoinRpcConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BitcoinRpcConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("password", &"[REDACTED]")
            .field("wallet_rescan_from_height", &self.wallet_rescan_from_height)
            .finish()
    }
}

/// Configuration for connecting to Esplora
#[derive(Clone)]
pub struct EsploraConfig {
    /// URL of the Esplora server endpoint
    pub url: String,
    /// Number of parallel requests to use during sync
    pub parallel_requests: usize,
}

impl fmt::Debug for EsploraConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EsploraConfig")
            .field("url", &url_for_logs(&self.url))
            .field("parallel_requests", &self.parallel_requests)
            .finish()
    }
}

/// Configuration for connecting to Electrum
#[derive(Clone)]
pub struct ElectrumConfig {
    /// URL of the Electrum server endpoint
    pub url: String,
    /// Number of scripts to request in each Electrum batch
    pub batch_size: usize,
}

impl fmt::Debug for ElectrumConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ElectrumConfig")
            .field("url", &url_for_logs(&self.url))
            .field("batch_size", &self.batch_size)
            .finish()
    }
}

/// Source of blockchain data for the BDK wallet
#[derive(Clone)]
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

impl fmt::Debug for ChainSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(feature = "esplora")]
            Self::Esplora(config) => f.debug_tuple("Esplora").field(config).finish(),
            #[cfg(feature = "electrum")]
            Self::Electrum(config) => f.debug_tuple("Electrum").field(config).finish(),
            #[cfg(feature = "bitcoin-rpc")]
            Self::BitcoinRpc(config) => f.debug_tuple("BitcoinRpc").field(config).finish(),
            #[allow(unreachable_patterns)]
            _ => f.write_str("ChainSource"),
        }
    }
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

    pub(crate) fn initial_checkpoint(&self) -> Result<Option<BlockId>, Error> {
        match self {
            #[cfg(feature = "bitcoin-rpc")]
            Self::BitcoinRpc(config) => bitcoin_rpc::initial_checkpoint(config).map(Some),
            #[allow(unreachable_patterns)]
            _ => Ok(None),
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "bitcoin-rpc")]
    #[test]
    fn bitcoin_rpc_debug_redacts_password() {
        let config = BitcoinRpcConfig {
            host: "127.0.0.1".to_string(),
            port: 8332,
            user: "rpc-user".to_string(),
            password: "rpc-password-secret".to_string(),
            wallet_rescan_from_height: Some(800_000),
        };

        let debug = format!("{config:?}");

        assert!(debug.contains("127.0.0.1"));
        assert!(debug.contains("rpc-user"));
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("rpc-password-secret"));
    }

    #[cfg(feature = "esplora")]
    #[test]
    fn esplora_debug_redacts_url_credentials() {
        let source = ChainSource::Esplora(EsploraConfig {
            url: "https://esplora-user:esplora-secret@example.com/api".to_string(),
            parallel_requests: 4,
        });

        let debug = format!("{source:?}");

        assert!(debug.contains("https://example.com/api"));
        assert!(!debug.contains("esplora-user"));
        assert!(!debug.contains("esplora-secret"));
    }

    #[cfg(feature = "electrum")]
    #[test]
    fn electrum_debug_redacts_url_credentials() {
        let source = ChainSource::Electrum(ElectrumConfig {
            url: "ssl://electrum-user:electrum-secret@example.com:50002".to_string(),
            batch_size: 5,
        });

        let debug = format!("{source:?}");

        assert!(debug.contains("ssl://example.com:50002"));
        assert!(!debug.contains("electrum-user"));
        assert!(!debug.contains("electrum-secret"));
    }

    #[cfg(feature = "electrum")]
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

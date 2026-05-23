use bdk_wallet::bitcoin::{OutPoint, Transaction};
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

    pub(crate) async fn broadcast(
        &self,
        tx: Transaction,
    ) -> Result<BroadcastOutcome, BroadcastFailure> {
        match self {
            #[cfg(feature = "esplora")]
            ChainSource::Esplora(config) => esplora::broadcast_esplora(config, tx).await,
            #[cfg(feature = "bitcoin-rpc")]
            ChainSource::BitcoinRpc(config) => bitcoin_rpc::broadcast_bitcoin_rpc(config, tx).await,
            #[allow(unreachable_patterns)]
            _ => unreachable!("ChainSource must have at least one feature enabled"),
        }
    }

    /// Dry-run whether a transaction would be accepted for broadcast.
    ///
    /// `Some(true/false)` via Bitcoin Core `testmempoolaccept`, or `None` when the
    /// backend has no dry-run (Esplora) and the caller should rely on a min-fee
    /// floor.
    #[cfg_attr(not(feature = "bitcoin-rpc"), allow(unused_variables))]
    pub(crate) async fn accepts_broadcast(&self, tx: &Transaction) -> Result<Option<bool>, Error> {
        match self {
            // Esplora has no `testmempoolaccept`; `None` tells the caller to fall
            // back to the min-fee-rate floor (see `is_payjoin_input_seen`).
            #[cfg(feature = "esplora")]
            ChainSource::Esplora(_) => Ok(None),
            #[cfg(feature = "bitcoin-rpc")]
            ChainSource::BitcoinRpc(config) => {
                bitcoin_rpc::accepts_broadcast_bitcoin_rpc(config, tx)
                    .await
                    .map(Some)
            }
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

    /// Return true when any provided outpoint is confirmed spent on chain.
    ///
    /// Mempool spends are intentionally ignored so cut-through recovery never
    /// releases a reserved melt on an unconfirmed conflict.
    pub(crate) async fn any_confirmed_spend(&self, outpoints: &[OutPoint]) -> Result<bool, Error> {
        match self {
            #[cfg(feature = "esplora")]
            ChainSource::Esplora(config) => {
                esplora::any_confirmed_spend_esplora(config, outpoints).await
            }
            #[cfg(feature = "bitcoin-rpc")]
            ChainSource::BitcoinRpc(config) => {
                bitcoin_rpc::any_confirmed_spend_bitcoin_rpc(config, outpoints).await
            }
            #[allow(unreachable_patterns)]
            _ => unreachable!("ChainSource must have at least one feature enabled"),
        }
    }
}

#[cfg(all(test, feature = "esplora"))]
mod tests {
    use bdk_wallet::bitcoin::absolute::LockTime;
    use bdk_wallet::bitcoin::transaction::Version;

    use super::*;

    #[tokio::test]
    async fn esplora_has_no_test_broadcast_capability() {
        // Esplora cannot dry-run accept-check, so it must report `None` so the
        // payjoin caller falls back to the minimum-fee-rate floor instead.
        let chain = ChainSource::Esplora(EsploraConfig {
            url: "https://example.invalid".to_string(),
            parallel_requests: 1,
        });
        let tx = Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![],
            output: vec![],
        };
        assert_eq!(chain.accepts_broadcast(&tx).await.unwrap(), None);
    }
}

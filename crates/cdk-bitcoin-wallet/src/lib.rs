//! Backend-neutral persisted BDK descriptor wallet and chain access.
//!
//! This crate owns reusable Bitcoin mechanics only. It has no Cashu payment
//! adapter and no Bark state machine; those backends consume it independently.

use bdk_wallet::rusqlite::Connection;
use bdk_wallet::PersistedWallet;

pub use self::chain::{
    BitcoinRpcConfig, BroadcastErrorKind, BroadcastFailure, BroadcastOutcome, ChainSource,
    ChainSourceKind, ElectrumConfig, EsploraConfig, DEFAULT_CHAIN_REQUEST_TIMEOUT_SECS,
    DEFAULT_ESPLORA_MAX_RETRIES, MAX_CHAIN_REQUEST_TIMEOUT_SECS, MAX_ESPLORA_MAX_RETRIES,
    MAX_ESPLORA_PARALLEL_REQUESTS,
};
pub use self::error::Error;
pub use self::federation::{
    FederatedBdkLocalConfig, FederatedBdkWallet, FederatedBlockMembership,
    FederatedChainSourceDescriptor, FederatedChainSourceScope, FederatedTransactionConfirmation,
    FederatedWalletConfig, FederatedWalletKeychain, FederatedWalletOutput,
    FederatedWalletOutputConfirmation, FederatedWalletReadiness, FederatedWalletScope,
    FederatedWalletSyncHealth, FederatedWalletSyncStatus,
};
pub use self::wallet_engine::{BdkWalletEngine, ConfirmedWalletOutput, DescriptorConfig};

/// Backend-neutral name for one isolated threshold descriptor wallet.
pub type FederatedDescriptorWallet = FederatedBdkWallet;
/// Backend-neutral accepted descriptor-wallet configuration.
pub type FederatedDescriptorWalletConfig = FederatedWalletConfig;
/// Backend-neutral member-local descriptor-wallet configuration.
pub type FederatedDescriptorWalletLocalConfig = FederatedBdkLocalConfig;
/// Backend-neutral immutable descriptor-wallet scope.
pub type FederatedDescriptorWalletScope = FederatedWalletScope;
/// Backend-neutral descriptor branch selected by an accepted allocation.
pub type FederatedDescriptorWalletKeychain = FederatedWalletKeychain;

/// Member-local chain-source adapters and classified broadcast outcomes.
pub mod chain;
pub mod error;
pub mod federation;
pub mod wallet_engine;

/// Persisted BDK wallet and its SQLite connection, locked as one unit.
#[doc(hidden)]
pub struct WalletWithDb {
    /// Persisted descriptor wallet.
    pub wallet: PersistedWallet<Connection>,
    /// SQLite persistence connection.
    pub db: Connection,
}

impl core::fmt::Debug for WalletWithDb {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("WalletWithDb")
            .field("wallet", &"<persisted descriptor wallet>")
            .field("db", &"<sqlite connection>")
            .finish()
    }
}

impl WalletWithDb {
    pub(crate) fn new(wallet: PersistedWallet<Connection>, db: Connection) -> Self {
        Self { wallet, db }
    }

    /// Persist staged wallet graph changes atomically with the shared connection.
    pub fn persist(&mut self) -> Result<bool, bdk_wallet::rusqlite::Error> {
        self.wallet.persist(&mut self.db)
    }
}

//! Wallet identity, construction, and seed-restore configuration.

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::{NUT13Options, Restored, Wallet};
use crate::mint_url::MintUrl;
use crate::nuts::CurrencyUnit;
use crate::Error;

/// Configuration required to open one standalone mint-and-unit wallet.
pub struct WalletOpenRequest {
    identity: WalletIdentity,
    store: Arc<dyn cdk_common::database::WalletDatabase<cdk_common::database::Error> + Send + Sync>,
    seed: [u8; 64],
    advanced: crate::wallet::advanced::WalletOpenAdvancedOptions,
}

impl fmt::Debug for WalletOpenRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WalletOpenRequest")
            .field("identity", &self.identity)
            .field("store", &"[CONFIGURED]")
            .field("seed", &"[REDACTED]")
            .field("advanced", &self.advanced)
            .finish()
    }
}

impl WalletOpenRequest {
    /// Open a wallet with default proof-management behavior.
    pub fn new(
        identity: WalletIdentity,
        store: Arc<
            dyn cdk_common::database::WalletDatabase<cdk_common::database::Error> + Send + Sync,
        >,
        seed: [u8; 64],
    ) -> Self {
        Self {
            identity,
            store,
            seed,
            advanced: crate::wallet::advanced::WalletOpenAdvancedOptions::default(),
        }
    }

    /// Apply explicitly advanced proof-management configuration.
    pub fn with_advanced(
        mut self,
        advanced: crate::wallet::advanced::WalletOpenAdvancedOptions,
    ) -> Self {
        self.advanced = advanced;
        self
    }
}

/// Mint and currency unit managed by one wallet.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WalletIdentity {
    /// Mint URL.
    pub mint_url: MintUrl,
    /// Currency unit.
    pub unit: CurrencyUnit,
}

impl WalletIdentity {
    /// Identify one mint-and-unit wallet.
    pub fn new(mint_url: MintUrl, unit: CurrencyUnit) -> Self {
        Self { mint_url, unit }
    }
}

impl fmt::Display for WalletIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.mint_url, self.unit)
    }
}

/// Seed-restore scan configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestoreRequest {
    /// Number of deterministic outputs requested per batch.
    pub batch_size: u32,
    /// Consecutive empty batches that terminate the scan.
    pub max_gap: u32,
}

impl Default for RestoreRequest {
    fn default() -> Self {
        let options = NUT13Options::default();
        Self {
            batch_size: options.batch_size,
            max_gap: options.max_gap,
        }
    }
}

impl Wallet {
    /// Open one standalone wallet without contacting its mint.
    pub fn open(request: WalletOpenRequest) -> Result<Self, Error> {
        crate::wallet::WalletBuilder::new()
            .with_mint_url(request.identity.mint_url)
            .with_unit(request.identity.unit)
            .with_store(request.store)
            .with_seed(request.seed)
            .with_target_proof_count(request.advanced.target_proof_count())
            .build()
    }

    /// Return this wallet's stable mint-and-unit identity.
    pub fn identity(&self) -> WalletIdentity {
        WalletIdentity {
            mint_url: self.mint_url.clone(),
            unit: self.unit.clone(),
        }
    }

    /// Re-scan deterministic wallet history from the seed.
    pub async fn restore_from_seed(&self, request: RestoreRequest) -> Result<Restored, Error> {
        let options = NUT13Options::new(request.batch_size, request.max_gap)?;
        self.restore_with_opts(options).await
    }
}

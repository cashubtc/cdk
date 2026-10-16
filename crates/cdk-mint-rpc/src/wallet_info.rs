//! On-chain wallet management provider.

use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;

use crate::wallet::{GetBalanceResponse, WalletAddress, WalletTransaction};

/// Error returned while managing the configured on-chain wallet.
#[derive(Debug, Error)]
#[error("{message}")]
pub struct WalletInfoError {
    message: String,
}

impl WalletInfoError {
    /// Creates an error from a backend-provided message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// A page of wallet transactions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletTransactionPage {
    /// Transactions in newest-first order.
    pub transactions: Vec<WalletTransaction>,
    /// Total transactions before pagination.
    pub total: u64,
}

/// A page of revealed wallet addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletAddressPage {
    /// Revealed addresses in derivation order.
    pub addresses: Vec<WalletAddress>,
    /// Total revealed addresses before pagination.
    pub total: u64,
}

/// Supplies operations for the management wallet service.
#[async_trait]
pub trait WalletInfoProvider {
    /// Creates a fresh address for operator deposits.
    async fn create_deposit_address(&self) -> Result<String, WalletInfoError>;

    /// Returns the wallet balance.
    async fn get_balance(&self) -> Result<GetBalanceResponse, WalletInfoError>;

    /// Returns a page of wallet transactions.
    async fn list_transactions(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<WalletTransactionPage, WalletInfoError>;

    /// Returns a page of revealed wallet addresses.
    async fn list_addresses(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<WalletAddressPage, WalletInfoError>;
}

/// Dynamically dispatched wallet information provider.
pub type DynWalletInfoProvider = Arc<dyn WalletInfoProvider + Send + Sync>;

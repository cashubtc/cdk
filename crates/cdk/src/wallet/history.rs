//! Application-facing transaction history.

use std::collections::HashMap;

use super::operation::OperationId;
use super::{Wallet, WalletIdentity, WalletManager};
use crate::nuts::PaymentMethod;
use crate::{Amount, Error};

/// Transaction-history filter.
#[derive(Debug, Clone, Copy, Default)]
pub struct HistoryQuery {
    /// Restrict results by direction.
    pub direction: Option<cdk_common::wallet::TransactionDirection>,
    /// Maximum number of newest entries to return.
    pub limit: Option<usize>,
}

/// Application-facing wallet history entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    /// Stable transaction identifier.
    pub id: cdk_common::wallet::TransactionId,
    /// Wallet that owns the transaction.
    pub wallet: WalletIdentity,
    /// Incoming or outgoing flow.
    pub direction: cdk_common::wallet::TransactionDirection,
    /// Principal value.
    pub amount: Amount,
    /// Fee charged to this wallet.
    pub fee: Amount,
    /// Unix timestamp.
    pub timestamp: u64,
    /// User-visible memo.
    pub memo: Option<String>,
    /// Application metadata.
    pub metadata: HashMap<String, String>,
    /// Related mint or melt quote identifier.
    pub quote_id: Option<String>,
    /// Durable operation identifier, when the transaction belongs to a workflow.
    pub operation_id: Option<OperationId>,
    /// Payment rail, when applicable.
    pub payment_method: Option<PaymentMethod>,
    /// Durable transaction status.
    pub status: cdk_common::wallet::TransactionStatus,
}

impl From<cdk_common::wallet::Transaction> for HistoryEntry {
    fn from(transaction: cdk_common::wallet::Transaction) -> Self {
        Self {
            id: transaction.id(),
            wallet: WalletIdentity {
                mint_url: transaction.mint_url,
                unit: transaction.unit,
            },
            direction: transaction.direction,
            amount: transaction.amount,
            fee: transaction.fee,
            timestamp: transaction.timestamp,
            memo: transaction.memo,
            metadata: transaction.metadata,
            quote_id: transaction.quote_id,
            operation_id: transaction.saga_id.map(Into::into),
            payment_method: transaction.payment_method,
            status: transaction.status,
        }
    }
}

impl Wallet {
    /// Read application-facing transaction history.
    pub async fn history(&self, query: HistoryQuery) -> Result<Vec<HistoryEntry>, Error> {
        let mut transactions = self.list_transactions(query.direction).await?;
        if let Some(limit) = query.limit {
            transactions.truncate(limit);
        }
        Ok(transactions.into_iter().map(Into::into).collect())
    }
}

impl WalletManager {
    /// Read application-facing history across all configured mint wallets.
    pub async fn history_all(&self, query: HistoryQuery) -> Result<Vec<HistoryEntry>, Error> {
        let mut transactions = self.list_transactions(query.direction).await?;
        if let Some(limit) = query.limit {
            transactions.truncate(limit);
        }
        Ok(transactions.into_iter().map(Into::into).collect())
    }
}

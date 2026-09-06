//! Application-level wallet events.
//!
//! These events describe durable workflow and balance changes without exposing
//! proof identifiers, protocol subscription filters, or mint wire messages.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::broadcast;

use super::history::HistoryEntry;
use super::mint::MintQuoteId;
use super::operation::{OperationKind, OperationReference, OperationState};
use super::{Wallet, WalletBalance, WalletIdentity};
use crate::Amount;

/// Application-facing snapshot of an operation transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationEvent {
    /// Stable operation or quote reference.
    pub reference: OperationReference,
    /// User-facing workflow kind.
    pub kind: OperationKind,
    /// New lifecycle state.
    pub state: OperationState,
    /// Principal value, when known.
    pub amount: Option<Amount>,
}

/// High-level change emitted by one wallet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WalletEvent {
    /// Spendable, pending, or reserved value changed.
    BalanceChanged {
        /// Wallet whose balance changed.
        wallet: WalletIdentity,
        /// Latest local balance snapshot.
        balance: WalletBalance,
    },
    /// A durable operation changed lifecycle state.
    OperationChanged {
        /// Wallet that owns the operation.
        wallet: WalletIdentity,
        /// Operation transition.
        operation: OperationEvent,
    },
    /// A transaction was created or changed status.
    TransactionChanged {
        /// Latest transaction snapshot.
        transaction: HistoryEntry,
    },
    /// An incoming quote received additional paid value.
    MintPaymentReceived {
        /// Wallet that owns the quote.
        wallet: WalletIdentity,
        /// Incoming quote.
        quote_id: MintQuoteId,
        /// Total value currently reported paid.
        amount_paid: Amount,
    },
}

/// Failure while reading an application event stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum WalletEventError {
    /// The receiver fell behind and one or more events were dropped.
    #[error("wallet event receiver lagged by {0} events")]
    Lagged(u64),
    /// Every wallet sender was dropped.
    #[error("wallet event stream closed")]
    Closed,
}

/// Independent receiver for one wallet's application events.
pub struct WalletEventReceiver {
    receiver: broadcast::Receiver<WalletEvent>,
}

impl fmt::Debug for WalletEventReceiver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WalletEventReceiver")
            .finish_non_exhaustive()
    }
}

impl WalletEventReceiver {
    pub(super) fn new(receiver: broadcast::Receiver<WalletEvent>) -> Self {
        Self { receiver }
    }

    /// Wait for the next application-level event.
    pub async fn next(&mut self) -> Result<WalletEvent, WalletEventError> {
        self.receiver.recv().await.map_err(|error| match error {
            broadcast::error::RecvError::Closed => WalletEventError::Closed,
            broadcast::error::RecvError::Lagged(count) => WalletEventError::Lagged(count),
        })
    }
}

impl Wallet {
    /// Subscribe to high-level changes produced by this wallet instance and
    /// all of its clones.
    pub fn events(&self) -> WalletEventReceiver {
        WalletEventReceiver::new(self.events.subscribe())
    }

    pub(crate) fn publish_operation_event(
        &self,
        reference: OperationReference,
        kind: OperationKind,
        state: OperationState,
        amount: Option<Amount>,
    ) {
        let _ = self.events.send(WalletEvent::OperationChanged {
            wallet: self.identity(),
            operation: OperationEvent {
                reference,
                kind,
                state,
                amount,
            },
        });
    }

    pub(crate) fn publish_mint_payment_event(&self, quote_id: MintQuoteId, amount_paid: Amount) {
        let _ = self.events.send(WalletEvent::MintPaymentReceived {
            wallet: self.identity(),
            quote_id,
            amount_paid,
        });
    }

    pub(crate) async fn publish_balance_event(&self) {
        match self.balance().await {
            Ok(balance) => {
                let _ = self.events.send(WalletEvent::BalanceChanged {
                    wallet: self.identity(),
                    balance,
                });
            }
            Err(error) => {
                tracing::warn!(%error, "Could not snapshot balance for wallet event");
            }
        }
    }

    pub(crate) async fn publish_transaction_events(&self, operation_id: uuid::Uuid) {
        match self.transactions_for_operation(operation_id).await {
            Ok(transactions) => {
                for transaction in transactions {
                    let _ = self.events.send(WalletEvent::TransactionChanged {
                        transaction: transaction.into(),
                    });
                }
            }
            Err(error) => {
                tracing::warn!(%error, %operation_id, "Could not load transaction for wallet event");
            }
        }
    }

    pub(crate) async fn publish_quote_transactions(&self, quote_id: &str) {
        match self.list_transactions(None).await {
            Ok(transactions) => {
                for transaction in transactions
                    .into_iter()
                    .filter(|transaction| transaction.quote_id.as_deref() == Some(quote_id))
                {
                    let _ = self.events.send(WalletEvent::TransactionChanged {
                        transaction: transaction.into(),
                    });
                }
            }
            Err(error) => {
                tracing::warn!(%error, %quote_id, "Could not load transaction for wallet event");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use cdk_common::wallet::{Transaction, TransactionDirection, TransactionStatus};

    use super::*;
    use crate::wallet::test_utils::{create_test_db, create_test_wallet};

    fn receive_transaction(
        wallet: &Wallet,
        operation_id: uuid::Uuid,
        timestamp: u64,
    ) -> Transaction {
        Transaction {
            mint_url: wallet.mint_url.clone(),
            direction: TransactionDirection::Incoming,
            amount: Amount::from(100),
            fee: Amount::ZERO,
            unit: wallet.unit.clone(),
            ys: vec![],
            timestamp,
            memo: None,
            metadata: HashMap::new(),
            quote_id: None,
            payment_request: None,
            payment_proof: None,
            payment_method: None,
            saga_id: Some(operation_id),
            status: TransactionStatus::Completed,
        }
    }

    #[tokio::test]
    async fn transaction_event_is_scoped_to_its_operation() {
        let wallet = create_test_wallet(create_test_db().await).await;
        let expected_id = uuid::Uuid::now_v7();
        let newer_id = uuid::Uuid::now_v7();
        wallet
            .upsert_transaction(receive_transaction(&wallet, expected_id, 1))
            .await
            .expect("store expected transaction");
        wallet
            .upsert_transaction(receive_transaction(&wallet, newer_id, 2))
            .await
            .expect("store newer transaction");

        let mut events = wallet.events();
        wallet.publish_transaction_events(expected_id).await;

        let WalletEvent::TransactionChanged { transaction } =
            events.next().await.expect("receive transaction event")
        else {
            panic!("expected a transaction event");
        };
        assert_eq!(transaction.operation_id, Some(expected_id.into()));
        assert!(events.receiver.try_recv().is_err());
    }
}

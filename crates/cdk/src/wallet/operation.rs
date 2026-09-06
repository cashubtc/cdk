//! Durable operation discovery and synchronization.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::mint::MintQuoteId;
use super::payment::PaymentQuoteId;
use super::{Wallet, WalletBalance, WalletIdentity, WalletManager};
use crate::nuts::{MeltQuoteState, MintQuoteState};
use crate::{Amount, Error};

/// Stable identifier for a durable wallet operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OperationId(Uuid);

impl OperationId {
    /// Create an identifier from its persisted UUID representation.
    pub const fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    /// Return the UUID representation used by wallet storage.
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl fmt::Display for OperationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<Uuid> for OperationId {
    fn from(value: Uuid) -> Self {
        Self(value)
    }
}

impl From<OperationId> for Uuid {
    fn from(value: OperationId) -> Self {
        value.0
    }
}

/// Stable reference to any resumable wallet operation.
///
/// Quote-backed sessions exist before a fund-reserving saga is created, so a
/// reference deliberately distinguishes quote identifiers from workflow
/// operation identifiers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "id", rename_all = "snake_case")]
pub enum OperationReference {
    /// Incoming mint quote.
    MintQuote(MintQuoteId),
    /// Outgoing payment quote.
    PaymentQuote(PaymentQuoteId),
    /// Fund-reserving durable workflow.
    Workflow(OperationId),
}

/// Application-level kind of a durable wallet operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    /// Incoming payment that issues ecash.
    Mint,
    /// Outgoing ecash token.
    Send,
    /// Incoming ecash token.
    Receive,
    /// Outgoing payment through a mint.
    Payment,
    /// Transfer between two mint wallets.
    Transfer,
    /// Protocol-level proof reissuance.
    Reissue,
}

/// Stable application-level lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    /// Waiting for the payer to fund an incoming quote.
    AwaitingPayment,
    /// A funded mint quote or outgoing payment quote can be acted on.
    Ready,
    /// A durable plan is waiting for an explicit execute or cancel decision.
    AwaitingExecution,
    /// Local or remote execution is in progress.
    Processing,
    /// A remote operation was accepted but has not reached a terminal state.
    Pending,
    /// The operation should be reconciled with [`Wallet::synchronize`].
    NeedsRecovery,
    /// The operation completed successfully.
    Completed,
    /// The operation was safely cancelled or compensated.
    Canceled,
    /// The operation reached a terminal failure.
    Failed,
}

/// Typed instruction for continuing a discovered operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum OperationResume {
    /// Load an incoming mint session with [`Wallet::resume_mint`].
    Mint {
        /// Quote to load.
        quote_id: MintQuoteId,
    },
    /// Load or inspect an ecash send with [`Wallet::resume_send`].
    Send {
        /// Workflow to load.
        operation_id: OperationId,
    },
    /// Load an outgoing quote with [`Wallet::resume_payment_quote`].
    PaymentQuote {
        /// Quote to load.
        quote_id: PaymentQuoteId,
    },
    /// Load a prepared or pending payment with its workflow identifier.
    Payment {
        /// Workflow to load.
        operation_id: OperationId,
        /// Quote owned by the workflow, when known.
        quote_id: Option<PaymentQuoteId>,
        /// Use [`Wallet::resume_pending_payment`] instead of
        /// [`Wallet::resume_payment`] when true.
        pending: bool,
    },
    /// Load a cross-mint transfer with [`WalletManager::resume_transfer`].
    Transfer {
        /// Source workflow to load.
        operation_id: OperationId,
    },
    /// Reconcile this operation with [`Wallet::synchronize`].
    Synchronize,
}

/// Application-facing summary of a resumable operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationSummary {
    /// Wallet that owns the quote or workflow.
    pub wallet: WalletIdentity,
    /// Wallet-local stable reference used for deduplication and persistence.
    pub reference: OperationReference,
    /// User-facing workflow kind.
    pub kind: OperationKind,
    /// Current high-level lifecycle state.
    pub state: OperationState,
    /// Principal value, when known.
    pub amount: Option<Amount>,
    /// Quote expiry as a Unix timestamp, when applicable.
    pub expires_at: Option<u64>,
    /// Creation time as a Unix timestamp, when recorded.
    pub created_at: Option<u64>,
    /// Last durable update as a Unix timestamp, when recorded.
    pub updated_at: Option<u64>,
    /// Typed continuation instruction.
    pub resume: OperationResume,
}

/// Filter for discovering durable wallet operations.
#[derive(Debug, Clone, Default)]
pub struct OperationQuery {
    /// Kinds to include. An empty vector includes every kind.
    pub kinds: Vec<OperationKind>,
    /// States to include. An empty vector includes every active state.
    pub states: Vec<OperationState>,
    /// Maximum number of newest results.
    pub limit: Option<usize>,
}

impl OperationQuery {
    /// Query every operation that still needs application attention.
    pub fn active() -> Self {
        Self::default()
    }

    /// Restrict the query to selected workflow kinds.
    pub fn with_kinds(mut self, kinds: Vec<OperationKind>) -> Self {
        self.kinds = kinds;
        self
    }

    /// Restrict the query to selected active states.
    pub fn with_states(mut self, states: Vec<OperationState>) -> Self {
        self.states = states;
        self
    }

    /// Limit the number of newest results.
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }
}

/// Result of reconciling one durable workflow during synchronization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationUpdate {
    /// Workflow or quote that was examined.
    pub reference: OperationReference,
    /// User-facing workflow kind.
    pub kind: OperationKind,
    /// State observed before recovery began.
    pub previous_state: OperationState,
    /// State after the recovery attempt.
    pub state: OperationState,
    /// Stable error category when reconciliation failed.
    pub error_kind: Option<cdk_common::error::WalletErrorKind>,
    /// Human-readable failure detail when reconciliation failed.
    pub error_message: Option<String>,
    /// Whether retrying after an external-state change may help.
    pub retryable: bool,
}

impl FromStr for OperationId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

/// Whether synchronization may contact the mint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum SyncPolicy {
    /// Inspect local durable state without network access.
    LocalOnly,
    /// Reconcile interrupted operations, quotes, and proofs with the mint.
    #[default]
    Online,
}

/// Result of one explicit wallet synchronization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncReport {
    /// Wallet reconciled by this pass.
    pub wallet: WalletIdentity,
    /// Balance after reconciliation.
    pub balance: WalletBalance,
    /// Interrupted operations completed successfully.
    pub recovered_operations: usize,
    /// Interrupted operations rolled back safely.
    pub compensated_operations: usize,
    /// Fund-reserving workflows that remain active after reconciliation.
    pub pending_operations: usize,
    /// Operations that could not be reconciled.
    pub failed_operations: usize,
    /// Paid mint quotes claimed during synchronization.
    pub claimed_amount: Amount,
    /// Orphaned value still reported unspent or pending by the mint.
    ///
    /// This value remains unavailable; synchronization does not reclaim it.
    pub unresolved_amount: Amount,
    /// Pending outgoing payments finalized during synchronization.
    pub finalized_payments: usize,
    /// Per-operation recovery results, including failures and retry guidance.
    pub operations: Vec<OperationUpdate>,
}

fn operation_kind_from_saga(
    saga: &cdk_common::wallet::WalletSaga,
    transfer_ids: &HashSet<Uuid>,
) -> OperationKind {
    use cdk_common::wallet::WalletSagaState;

    match saga.state {
        WalletSagaState::Send(_) => OperationKind::Send,
        WalletSagaState::Receive(_) => OperationKind::Receive,
        WalletSagaState::Swap(_) => OperationKind::Reissue,
        WalletSagaState::Issue(_) => OperationKind::Mint,
        WalletSagaState::Melt(_) if transfer_ids.contains(&saga.id) => OperationKind::Transfer,
        WalletSagaState::Melt(_) => OperationKind::Payment,
    }
}

fn operation_state_from_saga(state: cdk_common::wallet::WalletSagaState) -> OperationState {
    use cdk_common::wallet::{MeltSagaState, SendSagaState, WalletSagaState};

    match state {
        WalletSagaState::Send(SendSagaState::Prepared)
        | WalletSagaState::Melt(MeltSagaState::Prepared) => OperationState::AwaitingExecution,
        WalletSagaState::Send(SendSagaState::TokenCreated)
        | WalletSagaState::Melt(MeltSagaState::PaymentPending) => OperationState::Pending,
        WalletSagaState::Send(SendSagaState::RollingBack) => OperationState::NeedsRecovery,
        WalletSagaState::Send(_)
        | WalletSagaState::Receive(_)
        | WalletSagaState::Swap(_)
        | WalletSagaState::Issue(_)
        | WalletSagaState::Melt(_) => OperationState::Processing,
    }
}

fn operation_resume_from_saga(
    saga: &cdk_common::wallet::WalletSaga,
    kind: OperationKind,
) -> OperationResume {
    use cdk_common::wallet::{MeltSagaState, SendSagaState, WalletSagaState};

    let operation_id = OperationId::from(saga.id);
    match (kind, saga.state) {
        (OperationKind::Mint, _) => match saga.quote_id.as_ref() {
            Some(quote_id) => OperationResume::Mint {
                quote_id: MintQuoteId::new(quote_id.clone()),
            },
            None => OperationResume::Synchronize,
        },
        (
            OperationKind::Send,
            WalletSagaState::Send(SendSagaState::Prepared | SendSagaState::TokenCreated),
        ) => OperationResume::Send { operation_id },
        (OperationKind::Payment, WalletSagaState::Melt(state @ MeltSagaState::Prepared))
        | (OperationKind::Payment, WalletSagaState::Melt(state @ MeltSagaState::PaymentPending)) => {
            OperationResume::Payment {
                operation_id,
                quote_id: saga.quote_id.clone().map(PaymentQuoteId::new),
                pending: state == MeltSagaState::PaymentPending,
            }
        }
        (OperationKind::Transfer, WalletSagaState::Melt(MeltSagaState::Prepared))
        | (OperationKind::Transfer, WalletSagaState::Melt(MeltSagaState::PaymentPending)) => {
            OperationResume::Transfer { operation_id }
        }
        _ => OperationResume::Synchronize,
    }
}

fn operation_matches_query(operation: &OperationSummary, query: &OperationQuery) -> bool {
    (query.kinds.is_empty() || query.kinds.contains(&operation.kind))
        && (query.states.is_empty() || query.states.contains(&operation.state))
}

fn operation_sort_key(operation: &OperationSummary) -> u64 {
    operation
        .updated_at
        .or(operation.created_at)
        .or(operation.expires_at)
        .unwrap_or_default()
}

impl Wallet {
    /// Discover every locally durable operation that still needs attention.
    ///
    /// This includes quote-backed sessions that exist before a fund-reserving
    /// saga is created. Applications can therefore recover after a crash even
    /// if they did not persist the identifier returned by the original call.
    pub async fn operations(&self, query: OperationQuery) -> Result<Vec<OperationSummary>, Error> {
        let sagas = self
            .localstore
            .get_incomplete_sagas()
            .await?
            .into_iter()
            .filter(|saga| saga.mint_url == self.mint_url && saga.unit == self.unit)
            .collect::<Vec<_>>();
        let saga_ids = sagas.iter().map(|saga| saga.id).collect::<HashSet<_>>();
        let saga_quote_ids = sagas
            .iter()
            .filter_map(|saga| saga.quote_id.clone())
            .collect::<HashSet<_>>();

        let all_transfers = self.all_cross_mint_transfer_operations().await?;
        let transfers = all_transfers
            .iter()
            .filter(|operation| {
                operation.source_mint_url == self.mint_url && operation.source_unit == self.unit
            })
            .cloned()
            .collect::<Vec<_>>();
        let transfer_ids = transfers
            .iter()
            .map(|operation| operation.operation_id)
            .collect::<HashSet<_>>();
        let transfer_quote_ids = all_transfers
            .iter()
            .filter(|operation| {
                operation.destination_mint_url == self.mint_url
                    && operation.destination_unit == self.unit
            })
            .map(|operation| operation.destination_quote_id.clone())
            .collect::<HashSet<_>>();

        let mint_quotes = self.localstore.get_mint_quotes().await?;
        let melt_quotes = self.localstore.get_melt_quotes().await?;
        let now = cdk_common::util::unix_time();
        let mut operations = Vec::new();

        for saga in sagas {
            let kind = operation_kind_from_saga(&saga, &transfer_ids);
            let state = operation_state_from_saga(saga.state);
            let resume = operation_resume_from_saga(&saga, kind);
            operations.push(OperationSummary {
                wallet: self.identity(),
                reference: OperationReference::Workflow(saga.id.into()),
                kind,
                state,
                amount: Some(saga.amount),
                expires_at: None,
                created_at: Some(saga.created_at),
                updated_at: Some(saga.updated_at),
                resume,
            });
        }

        for quote in &mint_quotes {
            if quote.mint_url != self.mint_url
                || quote.unit != self.unit
                || saga_quote_ids.contains(&quote.id)
                || transfer_quote_ids.contains(&quote.id)
                || quote
                    .used_by_operation
                    .as_deref()
                    .and_then(|id| Uuid::parse_str(id).ok())
                    .is_some_and(|id| saga_ids.contains(&id))
            {
                continue;
            }
            let (state, amount) = match quote.state {
                MintQuoteState::Unpaid if quote.expiry > now => {
                    (OperationState::AwaitingPayment, quote.amount)
                }
                MintQuoteState::Paid => (OperationState::Ready, Some(quote.amount_mintable())),
                MintQuoteState::Unpaid | MintQuoteState::Issued => continue,
            };
            operations.push(OperationSummary {
                wallet: self.identity(),
                reference: OperationReference::MintQuote(MintQuoteId::new(quote.id.clone())),
                kind: OperationKind::Mint,
                state,
                amount,
                expires_at: Some(quote.expiry),
                created_at: None,
                updated_at: (quote.updated_at != 0).then_some(quote.updated_at),
                resume: OperationResume::Mint {
                    quote_id: MintQuoteId::new(quote.id.clone()),
                },
            });
        }

        for quote in melt_quotes {
            // Never guess ownership for legacy records without a mint URL: in
            // a shared multi-mint store, resuming one against the wrong mint
            // could target an unrelated quote with the same identifier.
            if quote.mint_url.as_ref() != Some(&self.mint_url)
                || quote.unit != self.unit
                || saga_quote_ids.contains(&quote.id)
                || quote
                    .used_by_operation
                    .as_deref()
                    .and_then(|id| Uuid::parse_str(id).ok())
                    .is_some_and(|id| saga_ids.contains(&id))
            {
                continue;
            }
            let state = match quote.state {
                MeltQuoteState::Unpaid if quote.expiry > now => OperationState::Ready,
                MeltQuoteState::Pending => OperationState::Pending,
                _ => continue,
            };
            operations.push(OperationSummary {
                wallet: self.identity(),
                reference: OperationReference::PaymentQuote(PaymentQuoteId::new(quote.id.clone())),
                kind: OperationKind::Payment,
                state,
                amount: Some(quote.amount),
                expires_at: Some(quote.expiry),
                created_at: None,
                updated_at: None,
                resume: OperationResume::PaymentQuote {
                    quote_id: PaymentQuoteId::new(quote.id),
                },
            });
        }

        for transfer in transfers {
            if saga_ids.contains(&transfer.operation_id) {
                continue;
            }
            let destination_is_issued = mint_quotes.iter().any(|quote| {
                quote.id == transfer.destination_quote_id
                    && quote.mint_url == transfer.destination_mint_url
                    && quote.unit == transfer.destination_unit
                    && quote.state == MintQuoteState::Issued
            });
            if destination_is_issued {
                continue;
            }
            operations.push(OperationSummary {
                wallet: self.identity(),
                reference: OperationReference::Workflow(transfer.operation_id.into()),
                kind: OperationKind::Transfer,
                state: OperationState::Pending,
                amount: Some(transfer.amount),
                expires_at: None,
                created_at: None,
                updated_at: None,
                resume: OperationResume::Transfer {
                    operation_id: transfer.operation_id.into(),
                },
            });
        }

        operations.retain(|operation| operation_matches_query(operation, &query));
        operations.sort_by_key(operation_sort_key);
        operations.reverse();
        if let Some(limit) = query.limit {
            operations.truncate(limit);
        }
        Ok(operations)
    }

    /// Explicitly reconcile wallet state.
    pub async fn synchronize(&self, policy: SyncPolicy) -> Result<SyncReport, Error> {
        let previous_operations = self.operations(OperationQuery::active()).await?;
        let (recovery, claimed_amount, unresolved_amount, finalized_payments) = match policy {
            SyncPolicy::LocalOnly => (
                crate::wallet::RecoveryReport::default(),
                Amount::ZERO,
                Amount::ZERO,
                0,
            ),
            SyncPolicy::Online => {
                let recovery = self.recover_incomplete_sagas().await?;
                let claimed_amount = self.mint_unissued_quotes().await?;
                let unresolved_amount = self.check_all_pending_proofs().await?;
                let finalized_payments = self.finalize_pending_melts().await?.len();
                (
                    recovery,
                    claimed_amount,
                    unresolved_amount,
                    finalized_payments,
                )
            }
        };
        let pending_operations = self
            .localstore
            .get_incomplete_sagas()
            .await?
            .into_iter()
            .filter(|saga| saga.mint_url == self.mint_url && saga.unit == self.unit)
            .count();

        let mut operation_updates = Vec::with_capacity(recovery.operations.len());
        for operation in recovery.operations {
            let previous = previous_operations.iter().find(|previous| {
                previous.reference == OperationReference::Workflow(operation.operation_id.into())
            });
            let kind = match previous {
                Some(previous) => previous.kind,
                None => match self
                    .cross_mint_transfer_operation(operation.operation_id)
                    .await?
                {
                    Some(_) => OperationKind::Transfer,
                    None => match operation.previous_state {
                        cdk_common::wallet::WalletSagaState::Send(_) => OperationKind::Send,
                        cdk_common::wallet::WalletSagaState::Receive(_) => OperationKind::Receive,
                        cdk_common::wallet::WalletSagaState::Swap(_) => OperationKind::Reissue,
                        cdk_common::wallet::WalletSagaState::Issue(_) => OperationKind::Mint,
                        cdk_common::wallet::WalletSagaState::Melt(_) => OperationKind::Payment,
                    },
                },
            };
            let previous_state = operation_state_from_saga(operation.previous_state);
            let state = match operation.action {
                Some(crate::wallet::recovery::RecoveryAction::Recovered) => {
                    OperationState::Completed
                }
                Some(crate::wallet::recovery::RecoveryAction::Compensated) => {
                    OperationState::Canceled
                }
                Some(crate::wallet::recovery::RecoveryAction::Skipped)
                    if matches!(
                        previous_state,
                        OperationState::AwaitingPayment
                            | OperationState::Ready
                            | OperationState::AwaitingExecution
                            | OperationState::Pending
                    ) =>
                {
                    previous_state
                }
                Some(crate::wallet::recovery::RecoveryAction::Skipped) => {
                    OperationState::NeedsRecovery
                }
                None if operation.retryable => OperationState::NeedsRecovery,
                None => OperationState::Failed,
            };
            operation_updates.push(OperationUpdate {
                reference: OperationReference::Workflow(operation.operation_id.into()),
                kind,
                previous_state,
                state,
                error_kind: operation.error_kind,
                error_message: operation.error_message,
                retryable: operation.retryable,
            });
        }

        let current_operations = self
            .operations(OperationQuery::active())
            .await?
            .into_iter()
            .map(|operation| (operation.reference.clone(), operation))
            .collect::<HashMap<_, _>>();
        let reported = operation_updates
            .iter()
            .map(|operation| operation.reference.clone())
            .collect::<HashSet<_>>();
        for previous in previous_operations {
            if reported.contains(&previous.reference) {
                continue;
            }
            operation_updates.push(OperationUpdate {
                reference: previous.reference,
                kind: previous.kind,
                previous_state: previous.state,
                // Disappearance from the active index is not proof of success.
                // Resolve the final state from durable records below.
                state: OperationState::NeedsRecovery,
                error_kind: None,
                error_message: None,
                retryable: false,
            });
        }
        self.reconcile_operation_updates(&mut operation_updates, &current_operations)
            .await?;

        let report = SyncReport {
            wallet: self.identity(),
            balance: self.balance().await?,
            recovered_operations: recovery.recovered,
            compensated_operations: recovery.compensated,
            pending_operations,
            failed_operations: operation_updates
                .iter()
                .filter(|update| update.error_kind.is_some())
                .count(),
            claimed_amount,
            unresolved_amount,
            finalized_payments,
            operations: operation_updates,
        };
        for operation in &report.operations {
            let changed =
                operation.state != operation.previous_state || operation.error_kind.is_some();
            if changed {
                self.publish_operation_event(
                    operation.reference.clone(),
                    operation.kind,
                    operation.state,
                    None,
                );
                match &operation.reference {
                    OperationReference::MintQuote(quote_id) => {
                        self.publish_quote_transactions(quote_id.as_str()).await;
                    }
                    OperationReference::PaymentQuote(quote_id) => {
                        self.publish_quote_transactions(quote_id.as_str()).await;
                    }
                    OperationReference::Workflow(operation_id) => {
                        self.publish_transaction_events(operation_id.as_uuid())
                            .await;
                    }
                }
            }
        }
        let _ = self
            .events
            .send(crate::wallet::events::WalletEvent::BalanceChanged {
                wallet: report.wallet.clone(),
                balance: report.balance,
            });
        Ok(report)
    }

    async fn reconcile_operation_updates(
        &self,
        updates: &mut [OperationUpdate],
        active: &HashMap<OperationReference, OperationSummary>,
    ) -> Result<(), Error> {
        use cdk_common::wallet::TransactionStatus;

        let now = cdk_common::util::unix_time();
        for update in updates {
            let state = match active.get(&update.reference) {
                Some(operation) => Some(operation.state),
                None => match &update.reference {
                    OperationReference::MintQuote(id) => self
                        .localstore
                        .get_mint_quote(id.as_str())
                        .await?
                        .filter(|quote| quote.mint_url == self.mint_url && quote.unit == self.unit)
                        .and_then(|quote| match quote.state {
                            MintQuoteState::Issued => Some(OperationState::Completed),
                            MintQuoteState::Unpaid if quote.expiry <= now => {
                                Some(OperationState::Failed)
                            }
                            _ => None,
                        }),
                    OperationReference::PaymentQuote(id) => self
                        .localstore
                        .get_melt_quote(id.as_str())
                        .await?
                        .filter(|quote| {
                            quote.mint_url.as_ref() == Some(&self.mint_url)
                                && quote.unit == self.unit
                        })
                        .and_then(|quote| match quote.state {
                            MeltQuoteState::Paid => Some(OperationState::Completed),
                            MeltQuoteState::Failed => Some(OperationState::Failed),
                            MeltQuoteState::Unpaid if quote.expiry <= now => {
                                Some(OperationState::Failed)
                            }
                            _ => None,
                        }),
                    OperationReference::Workflow(id) => self
                        .transactions_for_operation(id.as_uuid())
                        .await?
                        .into_iter()
                        .find_map(|transaction| match transaction.status {
                            TransactionStatus::Completed => Some(OperationState::Completed),
                            TransactionStatus::Failed => Some(OperationState::Failed),
                            TransactionStatus::Pending => None,
                        }),
                },
            };

            if let Some(state) = state {
                // Keep the explicit cancellation outcome from recovery. A
                // canceled send's transaction is stored as failed.
                if !(update.state == OperationState::Canceled && state == OperationState::Failed) {
                    update.state = state;
                }
                if matches!(state, OperationState::Completed | OperationState::Canceled) {
                    update.error_kind = None;
                    update.error_message = None;
                    update.retryable = false;
                } else if update.error_kind.is_some() && state == OperationState::Processing {
                    update.state = OperationState::NeedsRecovery;
                }
            }
        }
        Ok(())
    }
}

impl WalletManager {
    /// Discover active durable operations across every configured wallet.
    pub async fn operations(&self, query: OperationQuery) -> Result<Vec<OperationSummary>, Error> {
        let mut operations = Vec::new();
        for wallet in self.get_wallets().await {
            operations.extend(wallet.operations(query.clone()).await?);
        }
        operations.sort_by_key(operation_sort_key);
        operations.reverse();
        if let Some(limit) = query.limit {
            operations.truncate(limit);
        }
        Ok(operations)
    }

    /// Synchronize every configured mint wallet.
    pub async fn synchronize_all(&self, policy: SyncPolicy) -> Result<Vec<SyncReport>, Error> {
        let wallets = self.get_wallets().await;
        let mut reports = Vec::with_capacity(wallets.len());
        for wallet in wallets {
            reports.push(wallet.synchronize(policy).await?);
        }
        Ok(reports)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::nuts::{PaymentMethod, SecretKey, State};
    use crate::wallet::advanced::SendAdvancedOptions;
    use crate::wallet::events::WalletEvent;
    use crate::wallet::payment::{AddressPaymentRequest, PaymentTarget};
    use crate::wallet::test_utils::{
        create_test_db, create_test_wallet, test_mint_quote, test_mint_url,
    };

    fn operation_update(reference: OperationReference, kind: OperationKind) -> OperationUpdate {
        OperationUpdate {
            reference,
            kind,
            previous_state: OperationState::Ready,
            state: OperationState::NeedsRecovery,
            error_kind: None,
            error_message: None,
            retryable: false,
        }
    }

    #[tokio::test]
    async fn sync_uses_durable_issuance_before_quote_expiry() {
        let store = create_test_db().await;
        let wallet = create_test_wallet(Arc::clone(&store)).await;
        let mut quote = test_mint_quote(test_mint_url());
        quote.expiry = 1;
        let reference = OperationReference::MintQuote(MintQuoteId::new(quote.id.clone()));

        for (quote_state, expected) in [
            (MintQuoteState::Issued, OperationState::Completed),
            (MintQuoteState::Unpaid, OperationState::Failed),
        ] {
            quote.state = quote_state;
            store
                .add_mint_quote(quote.clone())
                .await
                .expect("persist quote");
            let mut updates = vec![operation_update(reference.clone(), OperationKind::Mint)];
            wallet
                .reconcile_operation_updates(&mut updates, &HashMap::new())
                .await
                .expect("resolve final quote state");
            assert_eq!(updates[0].state, expected);
        }

        store
            .remove_mint_quote(&quote.id)
            .await
            .expect("remove quote");
        let mut updates = vec![operation_update(reference, OperationKind::Mint)];
        wallet
            .reconcile_operation_updates(&mut updates, &HashMap::new())
            .await
            .expect("resolve missing quote");
        assert_eq!(updates[0].state, OperationState::NeedsRecovery);
    }

    #[tokio::test]
    async fn sync_uses_final_settlement_after_an_intermediate_recovery_result() {
        use cdk_common::wallet::{Transaction, TransactionDirection, TransactionStatus};

        let store = create_test_db().await;
        let wallet = create_test_wallet(Arc::clone(&store)).await;
        let id = Uuid::now_v7();
        store
            .add_transaction(Transaction {
                mint_url: wallet.mint_url.clone(),
                direction: TransactionDirection::Outgoing,
                amount: Amount::from(10),
                fee: Amount::ZERO,
                unit: wallet.unit.clone(),
                ys: vec![],
                timestamp: 1,
                memo: None,
                metadata: HashMap::new(),
                quote_id: Some("paid-quote".to_string()),
                payment_request: None,
                payment_proof: None,
                payment_method: Some(PaymentMethod::BOLT11),
                saga_id: Some(id),
                status: TransactionStatus::Completed,
            })
            .await
            .expect("persist settled transaction");

        let reference = OperationReference::Workflow(id.into());
        let mut update = operation_update(reference.clone(), OperationKind::Payment);
        update.previous_state = OperationState::Pending;
        update.state = OperationState::Pending;
        update.retryable = true;
        let mut updates = vec![update];
        wallet
            .reconcile_operation_updates(&mut updates, &HashMap::new())
            .await
            .expect("resolve final settlement");
        assert_eq!(updates[0].state, OperationState::Completed);
        assert!(!updates[0].retryable);

        // A settled source payment alone does not complete a cross-mint transfer.
        updates[0].kind = OperationKind::Transfer;
        let pending_transfer = OperationSummary {
            wallet: wallet.identity(),
            reference: reference.clone(),
            kind: OperationKind::Transfer,
            state: OperationState::Pending,
            amount: Some(Amount::from(10)),
            expires_at: None,
            created_at: None,
            updated_at: None,
            resume: OperationResume::Transfer {
                operation_id: id.into(),
            },
        };
        wallet
            .reconcile_operation_updates(
                &mut updates,
                &HashMap::from([(reference, pending_transfer)]),
            )
            .await
            .expect("resolve pending destination claim");
        assert_eq!(updates[0].state, OperationState::Pending);
    }

    #[tokio::test]
    async fn synchronize_does_not_report_unclaimed_value_as_recovered() {
        use crate::nuts::{CheckStateResponse, ProofState};
        use crate::wallet::test_utils::{
            create_test_wallet_with_mock, test_keyset_id, test_proof_info, MockMintConnector,
        };

        let store = create_test_db().await;
        let connector = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(Arc::clone(&store), Arc::clone(&connector)).await;
        let mut proof = test_proof_info(test_keyset_id(), 4, test_mint_url());
        proof.state = State::Pending;
        connector.set_check_state_response(Ok(CheckStateResponse {
            states: vec![ProofState {
                y: proof.y,
                state: State::Unspent,
                witness: None,
            }],
        }));
        store
            .update_proofs(vec![proof], vec![])
            .await
            .expect("persist orphaned value");

        let report = wallet
            .synchronize(SyncPolicy::Online)
            .await
            .expect("synchronize");
        assert_eq!(report.unresolved_amount, Amount::from(4));
        assert_eq!(report.balance.available, Amount::ZERO);
        assert_eq!(report.balance.pending, Amount::from(4));
    }

    #[allow(clippy::use_debug)]
    #[test]
    fn application_requests_redact_payment_and_signing_secrets() {
        let target = PaymentTarget::bolt11("lnbc-sensitive-invoice");
        let address =
            AddressPaymentRequest::lightning_address("alice@example.com", Amount::from(1_000));
        let signing_key = SecretKey::generate();
        let signing_key_hex = signing_key.to_secret_hex();
        let options = SendAdvancedOptions {
            p2pk_signing_keys: vec![signing_key],
            ..Default::default()
        };

        let target_output = format!("{target:?}");
        let address_output = format!("{address:?}");
        let options_output = format!("{options:?}");
        assert!(!target_output.contains("lnbc-sensitive-invoice"));
        assert!(!address_output.contains("alice@example.com"));
        assert!(!options_output.contains(&signing_key_hex));
        assert!(target_output.contains("[REDACTED]"));
        assert!(address_output.contains("[REDACTED]"));
        assert!(options_output.contains("[REDACTED]"));
    }

    #[tokio::test]
    async fn durable_quote_is_discoverable_resumable_and_reported_by_sync() {
        let store = create_test_db().await;
        let wallet = create_test_wallet(Arc::clone(&store)).await;
        let quote = test_mint_quote(test_mint_url());
        let quote_id = MintQuoteId::new(quote.id.clone());
        store
            .add_mint_quote(quote)
            .await
            .expect("persist test quote");

        let operations = wallet
            .operations(OperationQuery::active())
            .await
            .expect("discover durable operations");
        assert_eq!(operations.len(), 1);
        assert_eq!(operations[0].wallet, wallet.identity());
        assert_eq!(
            operations[0].reference,
            OperationReference::MintQuote(quote_id.clone())
        );
        assert_eq!(operations[0].kind, OperationKind::Mint);
        assert_eq!(operations[0].state, OperationState::AwaitingPayment);
        assert_eq!(
            operations[0].resume,
            OperationResume::Mint {
                quote_id: quote_id.clone()
            }
        );

        let payments = wallet
            .operations(OperationQuery::active().with_kinds(vec![OperationKind::Payment]))
            .await
            .expect("filter durable operations");
        assert!(payments.is_empty());

        let resumed = wallet
            .resume_mint(quote_id.clone())
            .await
            .expect("resume quote");
        assert_eq!(resumed.id(), &quote_id);

        let mut events = wallet.events();
        let report = wallet
            .synchronize(SyncPolicy::LocalOnly)
            .await
            .expect("locally synchronize wallet");
        assert_eq!(report.operations.len(), 1);
        assert_eq!(
            report.operations[0].reference,
            OperationReference::MintQuote(quote_id)
        );
        assert_eq!(
            report.operations[0].previous_state,
            OperationState::AwaitingPayment
        );
        assert_eq!(report.operations[0].state, OperationState::AwaitingPayment);

        match events.next().await.expect("receive synchronization event") {
            WalletEvent::BalanceChanged {
                wallet: owner,
                balance,
            } => {
                assert_eq!(owner, wallet.identity());
                assert_eq!(balance, report.balance);
            }
            event => panic!("unexpected wallet event: {event:?}"),
        }
    }

    #[tokio::test]
    async fn operation_index_reports_remaining_reusable_mint_value() {
        let store = create_test_db().await;
        let wallet = create_test_wallet(Arc::clone(&store)).await;
        let mut quote = test_mint_quote(test_mint_url());
        quote.payment_method = PaymentMethod::Custom("reusable".to_string());
        quote.amount = None;
        quote.state = MintQuoteState::Paid;
        quote.amount_paid = Amount::from(150);
        quote.amount_issued = Amount::from(30);
        store
            .add_mint_quote(quote)
            .await
            .expect("persist reusable quote");

        let operations = wallet
            .operations(OperationQuery::active())
            .await
            .expect("discover reusable quote");
        assert_eq!(operations.len(), 1);
        assert_eq!(operations[0].state, OperationState::Ready);
        assert_eq!(operations[0].amount, Some(Amount::from(120)));
    }

    #[tokio::test]
    async fn operation_index_does_not_duplicate_transfer_destination_quote() {
        let store = create_test_db().await;
        let wallet = create_test_wallet(Arc::clone(&store)).await;
        let operation_id = Uuid::now_v7();
        let mut quote = test_mint_quote(wallet.identity().mint_url.clone());
        quote.id = "transfer-destination-quote".to_string();
        store
            .add_mint_quote(quote.clone())
            .await
            .expect("persist destination quote");

        let transfer = cdk_common::wallet::CrossMintTransferOperation {
            operation_id,
            source_mint_url: "https://source.example.com"
                .parse()
                .expect("valid source mint URL"),
            source_unit: wallet.identity().unit.clone(),
            destination_mint_url: wallet.identity().mint_url,
            destination_unit: wallet.identity().unit,
            destination_quote_id: quote.id,
            amount: Amount::from(100),
            maximum_fee: Amount::from(2),
            allow_swap: true,
        };
        store
            .kv_write(
                "cdk_wallet",
                "cross_mint_transfers",
                &operation_id.to_string(),
                &serde_json::to_vec(&transfer).expect("serialize transfer"),
            )
            .await
            .expect("persist transfer");

        let operations = wallet
            .operations(OperationQuery::active())
            .await
            .expect("discover destination operations");
        assert!(operations.is_empty());
    }
}
